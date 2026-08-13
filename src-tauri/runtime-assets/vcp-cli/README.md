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

build/build_rootfs.sh
  -> android-assets/vcp-cli-rootfs-3.24.1-aarch64.tar.zst
```

PRoot 必须作为 Android 原生库随 APK 安装到只读/可执行的 `nativeLibraryDir`；不得在运行时下载
ELF 后从应用可写目录执行。rootfs archive 通过插件的专用 Android assets source set 打包，由应用
首次使用时解压到自己的私有数据目录。

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
- rootfs 解包后逻辑大小 107,258,068 bytes，zstd 资产 26,870,045 bytes；连同 PRoot 的 ZIP/APK 估算增量
  26,938,827 bytes。

这只是 P0/P1 前台执行证据，不等于 screen-off、Doze、划卡或 OEM 后台长稳验收。

## 许可证边界

本目录不是法律结论。PRoot 源文件声明 GPL-2.0-or-later，talloc 2.4.2 声明
LGPL-3.0-or-later；生成的独立 PRoot 可执行文件静态包含 talloc。Alpine rootfs 又是多个独立程序
的聚合，其中包含 GPL、LGPL、MIT、BSD、Apache、PSF 等许可证。发布时必须随 APK/Release
提供相应 notices、许可证文本和可重建的对应源码/对象，不得把仓库顶层许可证当作覆盖这些
第三方资产的单一许可证。详见 [`THIRD_PARTY.md`](./THIRD_PARTY.md)。
