# Android FFmpeg Binaries

This directory contains the cross-compiled `ffmpeg` and `ffprobe` binaries for Android `aarch64`.

They are embedded into the Rust binary at compile time via `include_bytes!` and extracted to the app's cache directory on first run.

## Build Environment

- **Host**: Windows 11 + WSL2 Ubuntu
- **Target**: `aarch64-linux-android`
- **NDK**: r26c (Linux x86_64 version)
- **ffmpeg**: 7.0
- **Why WSL**: ffmpeg's `configure` script requires a Unix shell environment. Windows NDK cannot be used directly in WSL because Windows `clang.exe` cannot access WSL paths (`/tmp/`, `/mnt/g/...`).

## Prerequisites

Inside WSL Ubuntu:

```bash
sudo apt-get update
sudo apt-get install -y make xz-utils python3
# yasm/nasm are NOT required for ARM64 cross-compilation
```

## Build Steps

### 1. Download Linux NDK

**Do NOT use the Windows NDK** (the one under `AppData\Local\Android\Sdk`).
Download the Linux version explicitly:

```bash
cd /tmp
wget https://dl.google.com/android/repository/android-ndk-r26c-linux.zip
python3 -m zipfile -e android-ndk-r26c-linux.zip ~
```

> **WARNING**: Python's `zipfile` module does NOT preserve Unix symlinks. Many files in the NDK `bin/` directory will be broken after extraction. See "Post-Extraction Fixes" below.

### 2. Download ffmpeg Source

```bash
mkdir -p /mnt/g/VCPMobile/temp && cd /mnt/g/VCPMobile/temp
wget https://ffmpeg.org/releases/ffmpeg-7.0.tar.xz
tar xf ffmpeg-7.0.tar.xz
```

### 3. Fix NDK Symlinks (Critical)

Python `zipfile` corrupts symlinks. Run this script to fix the broken links:

```bash
#!/bin/bash
NDK="$HOME/android-ndk-r26c"
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"

cd "$BIN"

# Remove broken self-links
rm -f clang clang++ llvm-ar llvm-strip llvm-objdump \
      llvm-readobj llvm-nm llvm-ranlib llc lld \
      ld.lld ld64.lld wasm-ld

# Re-link clang
ln -s clang-17 clang
ln -s clang-17 clang++

# Extract real lld binary from the zip (zipfile broke it into a 3-byte text file)
python3 -c "
import zipfile, os
z = zipfile.ZipFile('/tmp/android-ndk-r26c-linux.zip')
bindir = '$BIN'
with z.open('android-ndk-r26c/toolchains/llvm/prebuilt/linux-x86_64/bin/lld') as src:
    data = src.read()
    with open(f'{bindir}/lld', 'wb') as dst:
        dst.write(data)
    os.chmod(f'{bindir}/lld', 0o755)
"

# Re-link lld derivatives
ln -s lld ld.lld
ln -s lld ld64.lld
ln -s lld wasm-ld
ln -s ld.lld ld

echo "NDK symlinks fixed"
```

### 4. Cross-Compile ffmpeg

```bash
#!/bin/bash
set -e

NDK="$HOME/android-ndk-r26c"
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
PREFIX="/mnt/g/VCPMobile/temp/ffmpeg-output"
CC="$TOOLCHAIN/bin/aarch64-linux-android26-clang"
CXX="$TOOLCHAIN/bin/aarch64-linux-android26-clang++"
AR="/usr/bin/ar"

cd /mnt/g/VCPMobile/temp/ffmpeg-7.0

./configure \
  --prefix="$PREFIX" \
  --target-os=android --arch=aarch64 --cpu=armv8-a \
  --enable-cross-compile \
  --sysroot="$TOOLCHAIN/sysroot" \
  --cc="$CC" --cxx="$CXX" --ar="$AR" \
  --extra-cflags="-O3 -fPIC" \
  --extra-ldflags="-Wl,--gc-sections" \
  --disable-doc \
  --disable-avdevice \
  --disable-postproc \
  --disable-ffplay \
  --disable-encoders \
  --enable-encoder=mjpeg,pcm_s16le \
  --disable-muxers \
  --enable-muxer=image2,image2pipe,wav,null \
  --disable-decoders \
  --enable-decoder=h264,hevc,mjpeg,mpeg4,vp8,vp9,av1,mp3,aac,pcm_s16le,flac,vorbis,opus,png,webp,gif,bmp,tiff \
  --disable-demuxers \
  --enable-demuxer=mov,mp4,m4a,3gp,3g2,mj2,matroska,avi,flv,mp3,wav,ogg,flac,aac,image2,image2pipe \
  --disable-parsers \
  --enable-parser=h264,hevc,mjpeg,mpeg4video,vp8,vp9,aac,mpegaudio,vorbis,opus \
  --disable-debug \
  --disable-stripping \
  --enable-small

make -j$(nproc)
make install
```

> **Note**: `--disable-stripping` is required because the system `strip` is x86_64 and cannot process ARM64 ELF. We strip manually with `llvm-objcopy` afterwards.

### 5. Strip and Copy

```bash
cd "$PREFIX/bin"

# Strip symbols using NDK's llvm-objcopy
$TOOLCHAIN/bin/llvm-objcopy --strip-all ffmpeg ffmpeg_stripped
$TOOLCHAIN/bin/llvm-objcopy --strip-all ffprobe ffprobe_stripped
mv ffmpeg_stripped ffmpeg
mv ffprobe_stripped ffprobe

# Copy to project
cp ffmpeg  /mnt/g/VCPMobile/src-tauri/src/vcp_modules/media_processor/assets/ffmpeg_aarch64
cp ffprobe /mnt/g/VCPMobile/src-tauri/src/vcp_modules/media_processor/assets/ffprobe_aarch64
```

## Expected Output Size

| Binary | Unstripped | Stripped |
|--------|-----------|----------|
| ffmpeg | ~9.1 MB | **~7.6 MB** |
| ffprobe | ~9.0 MB | **~7.5 MB** |
| **Total** | ~18 MB | **~15 MB** |

APK compression (gzip) typically reduces this by ~60%, so the actual APK size increase is roughly **5-6 MB**.

## Feature-to-Usage Mapping

These options are pruned to match actual usage in `media_processor`:

| Feature | Used By | ffmpeg Flag |
|---------|---------|-------------|
| Scene detection | `detect_scene_changes` | `-vf select='gt(scene,0.3)',showinfo` |
| Video frame extraction | `process_video_for_multimodal` | `-vf fps=1,scale='min(1280,iw)':-1` |
| Audio extraction | `process_audio_for_multimodal` | `-acodec pcm_s16le -ar 16000 -ac 1 -f wav` |
| Large image scaling | `process_large_image_with_ffmpeg` | `-vf scale=... -vcodec mjpeg -f image2pipe` |
| ffprobe duration | `get_video_duration` | `-show_entries format=duration` |
| ffprobe streams | `get_video_info` | `-show_entries stream=...` |

## Common Pitfalls

1. **Do not use Windows NDK in WSL**: Windows `clang.exe` cannot read WSL paths. Always download the Linux NDK.
2. **Python `zipfile` breaks symlinks**: After extracting NDK with `zipfile`, `lld`, `clang`, and many other tools will be broken 3-byte text files. You must manually extract `lld` from the zip and recreate symlinks.
3. **System `ar` is fine for aarch64**: `ar` is just an archiver; x86_64 `ar` can pack aarch64 object files without issues.
4. **System `strip` cannot strip ARM64 ELF**: Use `--disable-stripping` during configure, then run `llvm-objcopy --strip-all` afterwards.
5. **No yasm/nasm needed for ARM64**: These are x86-only assembler tools. ARM64 cross-compilation does not require them.
