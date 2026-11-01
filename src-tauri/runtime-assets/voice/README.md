# 本地语音模型与裁剪运行库

此目录是 Android / iOS 本地 VAD 的共同构建输入。音频采集和 AEC 归平台原生 owner；这里不包含 ASR/TTS、网络推理或用户录音。

## 固定输入与运行契约

- Silero VAD `v6.2.2`：16kHz `silero_vad_16k_op15.onnx`，来源 commit、原始与派生文件 SHA-256 见 `runtime.lock.json`。
- ONNX Runtime `1.24.2`：固定 upstream commit；模型先以 `Fixed` + ARM 优化转换成 ORT flatbuffer，再生成**每个算子的类型白名单**。
- 每次输入 `input: float32[1,576]`（64 个历史 context + 512 个新样本）、`state: float32[2,1,128]` 和 `sr: int64 scalar=16000`；输出 `output` 与 `stateN`。断流/新会话必须清零 state/context。
- Session 使用 CPU、单线程、顺序执行、关闭线程自旋；优化已在导出时完成，原生会话禁用运行期 graph optimization。模型不适用于 8kHz 或多路 batch。
- Android 从 `assets/voice/silero_vad.ort` 加载。iOS 将共享的 `assets/voice` 作为主应用的 `voice` 目录资源嵌入，由 `Bundle.main` 加载。

## 裁剪范围

`build_runtime.py` 调用锁定上游源码的正式构建工具，使用 basic `--minimal_build`、`--include_ops_by_config`、`--enable_reduced_operator_type_support`、`MinSizeRel` 和 LTO。只保留 CPU，移除 ONNX 解析/运行期图优化、ML 类型、稀疏/Optional 类型、Float8/Float4、RTTI、KleidiAI 和 SVE；没有 CoreML、NNAPI、XNNPACK、训练和扩展执行提供方。

保留模型需要的 `FusedConv`，不能笼统禁用全部 contrib 算子。保留 C++ 异常，让模型/资源错误正常返回，避免用 `--disable_exceptions` 把错误改成进程 abort。没有量化模型或改变数值精度。

Android 核心 `libonnxruntime.so` 真正从以上配置编译；Java `classes.jar` 复用同版本、SHA-256 锁定的官方 AAR。JNI 桥也从锁定源码重新编译，只启用 CPU：官方桥硬链接 NNAPI 符号，不能直接搭配裁剪核心。构建后核对 JNI 所需 ORT 导出和两个 ELF 的 16KB LOAD 对齐。**官方完整推理核心和其他 ABI 不进入产物**。产物 AAR 在插件 Gradle 中解为本模块的 JAR/JNI 输入，避免 AGP 对 library 直接依赖本地 AAR 的打包限制。

iOS 使用同一算子/类型配置构建静态 `onnxruntime.xcframework`（仅 iphoneos arm64，最低 iOS 26），再从同一 commit 复制官方 Objective-C inference bindings，由本地 `onnxruntime_objc` SwiftPM target 编译。SwiftPM 的 binaryTarget 不会合入插件静态产物，因此脚本另外提取纯 arm64 的 `Runtime/libonnxruntime.a`，插件 `build.rs` 的 iOS 分支显式链接它以及 libc++、Objective-C runtime、Foundation。CPU MLAS 不需要 Accelerate。缺少裁剪产物会明确失败，不下载完整运行库兜底。Windows 不伪造 Apple framework，也不加入 macOS 产品目标。

## 复现

在独立 Python 3.11+ 环境安装 `requirements-build.txt`；该环境仅供构建和验数，不进入 App。CMake、Ninja、Git 与目标平台工具链应可执行；Android 还需要项目已有的 JDK 17（`javac`），用于从同版本 Java 源码生成 JNI 头。首次构建下载上游源码及其固定依赖，源与中间文件留在 `--cache-dir`，不进入仓库。

Android（Windows PowerShell 示例，SDK 路径按本机替换）：

```powershell
python -m pip install -r src-tauri/runtime-assets/voice/requirements-build.txt
python src-tauri/runtime-assets/voice/build_runtime.py android --android-sdk D:/Android/Sdk --cmake-bin D:/Android/Sdk/cmake/4.1.2/bin --jobs 8
```

只支持 Android `arm64-v8a`、API 26，NDK 固定 `29.0.14206865`。输出 `android/onnxruntime-silero-1.24.2.aar` 与 `android/build-report.json`，后者记录库/包实际字节数、哈希和裁剪参数。Android 普通构建使用已生成的 AAR，不临时下载或构建 ORT。

iOS（macOS + Xcode，必须先于 SwiftPM 解析）：

```sh
python3 -m pip install -r src-tauri/runtime-assets/voice/requirements-build.txt
python3 src-tauri/runtime-assets/voice/build_runtime.py ios --cache-dir /tmp/vcpmobile-silero-runtime --jobs 6
```

Apple 归档工作流先构建裁剪运行库，再由 SwiftPM 消费 `Runtime/` 和 `OrtObjC/`。原始模型派生产物、配置、许可证和构建入口在此目录维护。绑定使用 C++17，并显式从自建 `ios-arm64/onnxruntime.framework/Headers` 读取平铺头文件，不定义面向官方 pod 包目录布局的 `SPM_BUILD`。Xcode 26 下的 `swift-rs` 将 iOS target 传给编译器，但 SwiftPM 仍按 host triple 规划，不能依赖 binaryTarget 自动提供 iOS framework 搜索路径；核心静态库仍由 Cargo 显式链接。成功后生成 `ios/build-report.json`，记录实际 arm64 静态库与 XCFramework 大小和哈希。见 [iOS 构建说明](../../../docs/IOS_BUILD.md)。

已有锁定源码/构建目录可传 `--source-dir`、`--build-dir` 复用。`apple-inputs` 只复制 Apple 源码与资源，供 Windows 源码审计，**不产生 Apple 编译/链接证据**。

## 模型与产物验证

ORT 序列化可能重排等价图数据，重新导出的文件不保证逐字节稳定。因此日常构建使用已校验、锁定的 `.ort` 与算子配置。`--regenerate-model` 若得到不同字节会保留 cache 中候选并失败，不覆盖已验收资产；升级模型时应审查候选、重新验数，再显式更新资产与哈希，不能只改版本字符串。

host 等价、Android 交叉编译、Apple 编译与设备音频效果是四种不同证据。数值比较不证明裁剪库已经在手机运行；原生构建通过也不证明 AEC 有效。实际扬声器回声、双讲和路由切换须由平台音频链单独验证。

## 来源与许可证

Silero 与 ONNX Runtime 均保留上游 MIT 许可。`assets/voice/` 同时包含完整许可及 ONNX Runtime 上游第三方声明，随两端资源一起分发。

- [Silero 固定源码](https://github.com/snakers4/silero-vad/tree/60b7ffa243625ebdc1070275a29f18c87843786a)
- [ONNX Runtime 固定源码](https://github.com/microsoft/onnxruntime/tree/058787ceead760166e3c50a0a4cba8a833a6f53f)
- [官方裁剪构建说明](https://onnxruntime.ai/docs/build/custom.html)
