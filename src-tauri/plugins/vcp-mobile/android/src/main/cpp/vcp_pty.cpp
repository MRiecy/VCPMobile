#include <jni.h>

#include <cerrno>
#include <chrono>
#include <climits>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <mutex>
#include <poll.h>
#include <signal.h>
#include <string>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <thread>
#include <unistd.h>
#include <unordered_map>
#include <vector>

namespace {

constexpr size_t kMaxArgCount = 256;
constexpr size_t kMaxEnvCount = 64;
constexpr size_t kMaxWriteBytes = 16 * 1024;
constexpr size_t kMaxReadBytes = 64 * 1024;
constexpr jint kRunningExitCode = INT_MIN;

struct PtySession {
  int master_fd;
  pid_t pid;
  bool reaped = false;
  int exit_code = kRunningExitCode;
  std::mutex io_mutex;

  PtySession(int fd, pid_t child_pid) : master_fd(fd), pid(child_pid) {}
};

std::mutex g_sessions_mutex;
std::unordered_map<jlong, std::unique_ptr<PtySession>> g_sessions;
jlong g_next_handle = 1;

void throw_io(JNIEnv* env, const std::string& message) {
  jclass type = env->FindClass("java/io/IOException");
  if (type != nullptr) env->ThrowNew(type, message.c_str());
}

std::vector<std::string> string_array(JNIEnv* env, jobjectArray input, size_t limit) {
  const jsize count = env->GetArrayLength(input);
  if (count <= 0 || static_cast<size_t>(count) > limit) {
    throw_io(env, "PTY argument vector is outside the supported range");
    return {};
  }
  std::vector<std::string> output;
  output.reserve(static_cast<size_t>(count));
  for (jsize index = 0; index < count; ++index) {
    auto value = static_cast<jstring>(env->GetObjectArrayElement(input, index));
    if (value == nullptr) {
      throw_io(env, "PTY argument vector contains null");
      return {};
    }
    const char* utf = env->GetStringUTFChars(value, nullptr);
    if (utf == nullptr) return {};
    output.emplace_back(utf);
    env->ReleaseStringUTFChars(value, utf);
    env->DeleteLocalRef(value);
    if (output.back().find('\0') != std::string::npos || output.back().size() > 4096) {
      throw_io(env, "PTY argument exceeds its bounded string contract");
      return {};
    }
  }
  return output;
}

PtySession* find_session(jlong handle) {
  const auto found = g_sessions.find(handle);
  return found == g_sessions.end() ? nullptr : found->second.get();
}

int decoded_exit_status(int status) {
  if (WIFEXITED(status)) return WEXITSTATUS(status);
  if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
  return 255;
}

bool wait_for_exit(pid_t pid, int wait_ms, int* status) {
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(wait_ms);
  do {
    const pid_t result = waitpid(pid, status, WNOHANG);
    if (result == pid) return true;
    if (result < 0 && errno == ECHILD) return true;
    if (result < 0 && errno != EINTR) return false;
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
  } while (std::chrono::steady_clock::now() < deadline);
  return false;
}

}  // namespace

extern "C" JNIEXPORT jlongArray JNICALL
Java_com_vcp_mobile_cli_CliPtyNative_spawn(
    JNIEnv* env,
    jclass,
    jobjectArray argv_input,
    jobjectArray env_input,
    jint rows,
    jint cols,
    jlong address_space_kib) {
  if (rows <= 0 || rows > 1000 || cols <= 0 || cols > 1000 || address_space_kib <= 0) {
    throw_io(env, "PTY dimensions or memory budget are invalid");
    return nullptr;
  }
  auto argv_values = string_array(env, argv_input, kMaxArgCount);
  if (env->ExceptionCheck()) return nullptr;
  auto env_values = string_array(env, env_input, kMaxEnvCount);
  if (env->ExceptionCheck()) return nullptr;

  int master_fd = posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC);
  if (master_fd < 0 || grantpt(master_fd) != 0 || unlockpt(master_fd) != 0) {
    const int saved = errno;
    if (master_fd >= 0) close(master_fd);
    throw_io(env, std::string("Cannot allocate PTY: ") + strerror(saved));
    return nullptr;
  }
  char slave_name[PATH_MAX] = {};
  if (ptsname_r(master_fd, slave_name, sizeof(slave_name)) != 0) {
    const int saved = errno;
    close(master_fd);
    throw_io(env, std::string("Cannot resolve PTY slave: ") + strerror(saved));
    return nullptr;
  }
  struct winsize size {};
  size.ws_row = static_cast<unsigned short>(rows);
  size.ws_col = static_cast<unsigned short>(cols);
  if (ioctl(master_fd, TIOCSWINSZ, &size) != 0) {
    const int saved = errno;
    close(master_fd);
    throw_io(env, std::string("Cannot size PTY: ") + strerror(saved));
    return nullptr;
  }

  int error_pipe[2] = {-1, -1};
  if (pipe2(error_pipe, O_CLOEXEC) != 0) {
    const int saved = errno;
    close(master_fd);
    throw_io(env, std::string("Cannot create PTY exec fence: ") + strerror(saved));
    return nullptr;
  }

  const pid_t pid = fork();
  if (pid == 0) {
    close(error_pipe[0]);
    auto fail = [&](int code) {
      const int saved = errno;
      (void)!write(error_pipe[1], &saved, sizeof(saved));
      _exit(code);
    };
    prctl(PR_SET_PDEATHSIG, SIGHUP);
    if (setsid() < 0) fail(120);
    const int slave_fd = open(slave_name, O_RDWR | O_NOCTTY);
    if (slave_fd < 0 || ioctl(slave_fd, TIOCSCTTY, 0) != 0) fail(121);
    if (dup2(slave_fd, STDIN_FILENO) < 0 || dup2(slave_fd, STDOUT_FILENO) < 0 ||
        dup2(slave_fd, STDERR_FILENO) < 0) {
      fail(122);
    }
    if (slave_fd > STDERR_FILENO) close(slave_fd);
    close(master_fd);

    struct rlimit memory_limit {};
    memory_limit.rlim_cur = static_cast<rlim_t>(address_space_kib) * 1024;
    memory_limit.rlim_max = memory_limit.rlim_cur;
    if (setrlimit(RLIMIT_AS, &memory_limit) != 0) fail(123);

    clearenv();
    for (const auto& item : env_values) {
      const size_t separator = item.find('=');
      if (separator == std::string::npos || separator == 0) fail(124);
      const std::string key = item.substr(0, separator);
      const std::string value = item.substr(separator + 1);
      if (setenv(key.c_str(), value.c_str(), 1) != 0) fail(124);
    }
    std::vector<char*> argv;
    argv.reserve(argv_values.size() + 1);
    for (auto& item : argv_values) argv.push_back(item.data());
    argv.push_back(nullptr);
    execv(argv[0], argv.data());
    fail(127);
  }

  close(error_pipe[1]);
  if (pid < 0) {
    const int saved = errno;
    close(error_pipe[0]);
    close(master_fd);
    throw_io(env, std::string("Cannot fork PTY process: ") + strerror(saved));
    return nullptr;
  }

  struct pollfd exec_poll {error_pipe[0], POLLIN | POLLHUP, 0};
  const int poll_result = poll(&exec_poll, 1, 3000);
  if (poll_result <= 0) {
    const int saved = errno;
    close(error_pipe[0]);
    kill(-pid, SIGKILL);
    kill(pid, SIGKILL);
    int status = 0;
    (void)waitpid(pid, &status, 0);
    close(master_fd);
    if (poll_result == 0) {
      throw_io(env, "PTY child exec handshake timed out");
    } else {
      throw_io(env, std::string("PTY child exec handshake failed: ") + strerror(saved));
    }
    return nullptr;
  }

  int child_error = 0;
  const ssize_t error_bytes = read(error_pipe[0], &child_error, sizeof(child_error));
  const int read_error = errno;
  close(error_pipe[0]);
  if (error_bytes != 0) {
    kill(-pid, SIGKILL);
    kill(pid, SIGKILL);
    int status = 0;
    (void)waitpid(pid, &status, 0);
    close(master_fd);
    const int failure = error_bytes > 0 ? child_error : read_error;
    throw_io(env, std::string("PTY child exec failed: ") + strerror(failure));
    return nullptr;
  }

  const int current_flags = fcntl(master_fd, F_GETFL, 0);
  if (current_flags >= 0) (void)fcntl(master_fd, F_SETFL, current_flags | O_NONBLOCK);
  jlong handle = 0;
  {
    std::lock_guard<std::mutex> lock(g_sessions_mutex);
    handle = g_next_handle++;
    g_sessions.emplace(handle, std::make_unique<PtySession>(master_fd, pid));
  }
  jlong values[2] = {handle, static_cast<jlong>(pid)};
  jlongArray result = env->NewLongArray(2);
  if (result != nullptr) env->SetLongArrayRegion(result, 0, 2, values);
  return result;
}

extern "C" JNIEXPORT jbyteArray JNICALL
Java_com_vcp_mobile_cli_CliPtyNative_read(JNIEnv* env, jclass, jlong handle, jint max_bytes, jint wait_ms) {
  if (max_bytes <= 0 || static_cast<size_t>(max_bytes) > kMaxReadBytes || wait_ms < 0 || wait_ms > 1000) {
    throw_io(env, "PTY read budget is invalid");
    return nullptr;
  }
  std::lock_guard<std::mutex> sessions_lock(g_sessions_mutex);
  PtySession* session = find_session(handle);
  if (session == nullptr) {
    throw_io(env, "PTY handle is stale");
    return nullptr;
  }
  std::lock_guard<std::mutex> io_lock(session->io_mutex);
  struct pollfd descriptor {session->master_fd, POLLIN | POLLHUP | POLLERR, 0};
  const int ready = poll(&descriptor, 1, wait_ms);
  if (ready == 0) return nullptr;
  if (ready < 0 && errno == EINTR) return nullptr;
  if (ready < 0) {
    throw_io(env, std::string("PTY poll failed: ") + strerror(errno));
    return nullptr;
  }
  std::vector<jbyte> buffer(static_cast<size_t>(max_bytes));
  const ssize_t count = read(session->master_fd, buffer.data(), buffer.size());
  if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR)) return nullptr;
  if (count < 0 && errno != EIO) {
    throw_io(env, std::string("PTY read failed: ") + strerror(errno));
    return nullptr;
  }
  const jsize output_size = count > 0 ? static_cast<jsize>(count) : 0;
  jbyteArray output = env->NewByteArray(output_size);
  if (output != nullptr && output_size > 0) env->SetByteArrayRegion(output, 0, output_size, buffer.data());
  return output;
}

extern "C" JNIEXPORT jint JNICALL
Java_com_vcp_mobile_cli_CliPtyNative_write(JNIEnv* env, jclass, jlong handle, jbyteArray input) {
  const jsize length = env->GetArrayLength(input);
  if (length <= 0 || static_cast<size_t>(length) > kMaxWriteBytes) {
    throw_io(env, "PTY write budget is invalid");
    return 0;
  }
  std::vector<jbyte> bytes(static_cast<size_t>(length));
  env->GetByteArrayRegion(input, 0, length, bytes.data());
  std::lock_guard<std::mutex> sessions_lock(g_sessions_mutex);
  PtySession* session = find_session(handle);
  if (session == nullptr) {
    throw_io(env, "PTY handle is stale");
    return 0;
  }
  std::lock_guard<std::mutex> io_lock(session->io_mutex);
  size_t offset = 0;
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
  while (offset < bytes.size()) {
    const ssize_t written = write(
        session->master_fd,
        bytes.data() + offset,
        bytes.size() - offset);
    if (written > 0) {
      offset += static_cast<size_t>(written);
      continue;
    }
    if (written < 0 && errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR) {
      throw_io(env, std::string("PTY write failed: ") + strerror(errno));
      return 0;
    }
    const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
        deadline - std::chrono::steady_clock::now());
    if (remaining.count() <= 0) {
      throw_io(env, "PTY write timed out");
      return 0;
    }
    struct pollfd descriptor {session->master_fd, POLLOUT | POLLERR | POLLHUP, 0};
    const int ready = poll(&descriptor, 1, static_cast<int>(remaining.count()));
    if (ready < 0 && errno == EINTR) continue;
    if (ready <= 0 || (descriptor.revents & (POLLERR | POLLHUP)) != 0) {
      throw_io(env, ready == 0 ? "PTY write timed out" : "PTY became unavailable during write");
      return 0;
    }
  }
  return static_cast<jint>(offset);
}

extern "C" JNIEXPORT void JNICALL
Java_com_vcp_mobile_cli_CliPtyNative_resize(JNIEnv* env, jclass, jlong handle, jint rows, jint cols) {
  if (rows <= 0 || rows > 1000 || cols <= 0 || cols > 1000) {
    throw_io(env, "PTY dimensions are invalid");
    return;
  }
  std::lock_guard<std::mutex> sessions_lock(g_sessions_mutex);
  PtySession* session = find_session(handle);
  if (session == nullptr) {
    throw_io(env, "PTY handle is stale");
    return;
  }
  struct winsize size {};
  size.ws_row = static_cast<unsigned short>(rows);
  size.ws_col = static_cast<unsigned short>(cols);
  if (ioctl(session->master_fd, TIOCSWINSZ, &size) != 0) {
    throw_io(env, std::string("PTY resize failed: ") + strerror(errno));
    return;
  }
  (void)kill(-session->pid, SIGWINCH);
}

extern "C" JNIEXPORT jint JNICALL
Java_com_vcp_mobile_cli_CliPtyNative_exitCode(JNIEnv* env, jclass, jlong handle) {
  std::lock_guard<std::mutex> sessions_lock(g_sessions_mutex);
  PtySession* session = find_session(handle);
  if (session == nullptr) {
    throw_io(env, "PTY handle is stale");
    return kRunningExitCode;
  }
  if (session->reaped) return session->exit_code;
  int status = 0;
  const pid_t result = waitpid(session->pid, &status, WNOHANG);
  if (result == 0) return kRunningExitCode;
  if (result < 0 && errno == ECHILD) {
    session->reaped = true;
    session->exit_code = 255;
    return session->exit_code;
  }
  if (result < 0) {
    throw_io(env, std::string("PTY wait failed: ") + strerror(errno));
    return kRunningExitCode;
  }
  session->reaped = true;
  session->exit_code = decoded_exit_status(status);
  return session->exit_code;
}

extern "C" JNIEXPORT void JNICALL
Java_com_vcp_mobile_cli_CliPtyNative_close(JNIEnv* env, jclass, jlong handle) {
  std::unique_ptr<PtySession> session;
  {
    std::lock_guard<std::mutex> lock(g_sessions_mutex);
    const auto found = g_sessions.find(handle);
    if (found == g_sessions.end()) return;
    session = std::move(found->second);
    g_sessions.erase(found);
  }
  int status = 0;
  if (!session->reaped) {
    (void)kill(-session->pid, SIGHUP);
    (void)kill(-session->pid, SIGTERM);
    if (!wait_for_exit(session->pid, 1000, &status)) {
      (void)kill(-session->pid, SIGKILL);
      (void)kill(session->pid, SIGKILL);
      if (!wait_for_exit(session->pid, 2000, &status)) {
        throw_io(env, "PTY process group did not terminate");
      }
    }
  }
  close(session->master_fd);
}
