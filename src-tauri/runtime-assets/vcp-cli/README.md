# VCP Mobile CLI runtime assets

这里保存 `VCPMobileCLI` 的 Android arm64 运行环境合同与可复现构建输入。产品运行时只认
[`command-profile.json`](./command-profile.json) 中的单一 profile；manifest、实机探测和随 APK
分发的二进制必须与它一致。

## 冻结环境

- Android App UID，非 Root；PRoot guest 内的 `uid=0` 只是文件系统模拟。
- Alpine Linux 3.24.1（musl 1.2.6），GNU Bash 5.3.9。
- Agent 命令固定使用 `/bin/bash -lc <command>`，默认工作目录 `/workspace`。
- PRoot 来自 Termux 官方 `v5.1.107.89`，未采用 OpenMinis fork、native offload 或 SkillBridge。
- 80 个已安装 Alpine 包、72 个增量 APK 的版本、许可证和 SHA-256 固定在
  [`alpine-packages.lock.tsv`](./alpine-packages.lock.tsv)。

## 资产与构建

```text
build/build_proot.sh
  -> ../../plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot.so
  -> ../../plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot_loader.so

build/build_rootfs.sh
  -> android-assets/vcp-cli-rootfs-3.24.1-aarch64.tar.zst
```

PRoot 主程序和它的 unbundled loader 必须作为两个 Android 原生资产随 APK 安装到只读/可执行的
`nativeLibraryDir`；不得在运行时下载、释放或复制 ELF 后从应用可写目录执行。主程序以
`PROOT_UNBUNDLE_LOADER` 构建，编译期 fallback 故意指向不存在目录；`ProcessBuilder` 清空宿主环境后
显式设置 `PROOT_LOADER=<nativeLibraryDir>/libvcp_proot_loader.so`。guest 仍通过 `/usr/bin/env -i`
获得独立的固定环境。rootfs archive 通过插件的专用 Android assets source set 打包，由应用首次使用时
解压到自己的私有数据目录。

离线语义模型同样走专用 Android assets source set，但不会随普通 CLI provision 复制；只有首次
`river=semantic:N` 才由独立内部 bridge 按 [`semantic-profile.json`](./semantic-profile.json) 的
size/SHA-256 原子复制到 app-private assets 目录。Rust 随后以 mmap 只读加载模型和紧凑 BPE pack，
不启动 Python/ONNX/daemon，也不访问网络。构建期复现入口为
[`build/build_semantic_assets.py`](./build/build_semantic_assets.py)；脚本逐字校验上游 `model.safetensors`、
`tokenizer.json`、`config.json`，并拒绝 pre-tokenizer、AddedToken、padding/decoder 或 BPE flags 漂移。
同一 App 进程内，完整 asset identity 验证成功后复用缓存；进程重启会重新校验，损坏文件会原子修复。

冻结的离线语义资产身份：

| Asset | Bytes | SHA-256 |
|---|---:|---|
| `vcp-semantic-model-r2.safetensors` | 24,471,328 | `3a416974fe644efa62c0d33970a6403b2a00e0943d376e06dfc1dae85456b10b` |
| `vcp-semantic-tokenizer-r2.vcpbpe` | 10,437,027 | `4590cc5f76646fc2ebb5d6983ff445215b5f4ed5e5c9e81b171963f8b9a59e26` |

冻结的 Android arm64 ELF 身份：

| Asset | Bytes | SHA-256 |
|---|---:|---|
| `libvcp_proot.so` | 256,456 | `0d7168f851b42b83f7a75835cdb3a62181d12185620a1354038174706b0f367c` |
| `libvcp_proot_loader.so` | 17,728 | `cb5e5b6900e198ca8160e9d355ea5b98d646333887a769411ff74132c1cec5df` |

构建脚本会拒绝 hash/size 漂移、非 AArch64 ELF、主程序非 PIE、loader 非 `ET_EXEC`、任一 ELF 出现
可写且可执行的 `LOAD` segment、主程序缺少 unbundled fallback 或仍含 bundled loader 释放路径。

`build_rootfs.sh` 先校验 Alpine base 和每个 APK 的 SHA-256，删除会记录随机构建路径与时间的
`apk.log`，先落盘并校验 owner/mtime/order 固定的 tar，再以单线程 zstd 压缩。禁止直接把 tar
通过管道送入 zstd，因为上游分块边界会改变压缩帧字节。
profile 中的 `rootfs.tarContentSha256` 是与 zstd 版本无关的内容身份；发行资产的压缩字节 hash 也
单独固定。构建脚本遇到源包消失、hash 漂移或命令 profile 不匹配时必须失败，不能升级到
`latest`。

## 实机证据

2026-08-13 在 Android 16 / API 36 / arm64-v8a 真机完成：

- 官方 Termux PRoot 启动 `/bin/bash -lc`，Bash 5.3.9、Alpine 3.24.1；
- [`probe-command-profile.sh`](./probe-command-profile.sh) 中全部基线命令存在；
- `setsid -> PRoot -> Bash -> sleep` 的 PRoot、Bash、子进程处于同一目标 PGID；对负 PGID 发
  `SIGTERM` 后整棵进程树退出，PRoot 回执为 signal 15；
- rootfs 解包后逻辑大小 107,258,068 bytes，zstd 资产 26,870,045 bytes。

unbundled W^X 版本的三个原始资产共 27,144,229 bytes；以固定文件名和 `zip -9` 得到的 APK 增量估算
为 26,937,424 bytes。NDK 29 的独立重建已逐字复现上表两个 ELF；API 36 产品 `ProcessBuilder` 对
unbundled loader、取消以及 detached tracee 的复验仍是发布前真机门，不能由 JVM/构建检查替代。

这只是 P0/P1 前台执行证据，不等于 screen-off、Doze、划卡或 OEM 后台长稳验收。

2026-08-14 的 API 36/arm64 独立进程 feasibility 中，紧凑 pack + mmap 首次峰值 18,748 KiB、
总时长 51.8 ms，热运行峰值 16,376–16,536 KiB、总时长 16.7–27.1 ms；它只证明模型路径可行，
不替代产品 App PSS、首次索引、连续 50 次召回、低存储/损坏回退、温升、API 26 或代表 OEM 验收。

同日 `assembleArm64Debug` 的实际 Debug APK 为 84,528,816 bytes；ZIP 中 model/tokenizer 条目分别为
22,368,109 和 5,666,724 bytes，合计 28,034,833 bytes。解包后两文件的 size/SHA-256 与上表一致。
这是 Debug 包接线/体积证据，不代表 Release 包大小、安装后磁盘峰值或设备资源验收。

## 许可证边界

本目录不是法律结论。PRoot 源文件声明 GPL-2.0-or-later，talloc 2.4.2 声明
LGPL-3.0-or-later；生成的独立 PRoot 可执行文件静态包含 talloc。Alpine rootfs 又是多个独立程序
的聚合，其中包含 GPL、LGPL、MIT、BSD、Apache、PSF 等许可证。发布时必须随 APK/Release
提供相应 notices、许可证文本和可重建的对应源码/对象，不得把仓库顶层许可证当作覆盖这些
第三方资产的单一许可证。详见 [`THIRD_PARTY.md`](./THIRD_PARTY.md)。
