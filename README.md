# OpenShareMouse

在同一局域网内的多台电脑之间共享键盘、鼠标、剪贴板和文件。功能目标对标 ShareMouse Professional，使用自有协议，不与原版互通。

产品规格、架构和验收基线见 [ShareMouse-Professional-Reconstruction.md](ShareMouse-Professional-Reconstruction.md)（下文简称“报告”）。

**状态：v1。** 两台 Mac 通过一根雷雳数据线共用一套键盘鼠标，并可通过屏幕边缘的传送通道互传文件。

## v1 功能

- 检测本机雷雳端口：哪个端口接了设备、链路速率、对端是什么电脑、是否支持雷雳网络。
- 检测“雷雳网桥”（`bridge0`）是否启用、是否有地址。
- 两台 Mac 上都运行本应用后，通过雷雳网桥自动发现对方并连接。只走雷雳数据线，不经过 Wi-Fi。
- 把鼠标移出屏幕边缘（左或右，可设置）就会切到对方电脑，键盘随之跟过去。从对方屏幕的相对一侧移回来，就回到本机。
- 安全退出：
  - 按 ⌃⌘⎋（Control-Command-Esc）立即回到本机。
  - 拔掉数据线或对方超过 3 秒无响应时，自动回到本机，并松开对方电脑上被按住的键。
  - 被控制的电脑上有人点击、按键或滚动时，立即收回控制权。
- 主窗口显示数据线状态、端口信息、对方电脑信息、延迟和当前控制状态。
- 控制对方电脑期间，本机的 Fn 键会转发给对方，不在本机触发（例如微信输入法的 Fn 语音输入会在对方电脑上启动）。
- 文件传送通道：把本机文件拖到朝向对方的屏幕边缘中部，松手后复制到对方的下载目录（Downloads）。支持两个方向、多个文件和整个文件夹，文件内容分块传输。

### 文件传送通道

1. 两台电脑都安装当前版本并连接。左边电脑把“对方电脑位于本机的”设为右侧，右边电脑设为左侧。
2. 左边电脑的右边缘、右边电脑的左边缘，各出现一个“传送通道”。多显示器时放在对应方向最外侧显示器的中间。
3. 从访达将文件或文件夹拖入**文件所在电脑的通道**，在通道内松手。看到复制光标和“松手开始复制”时即可投放；每次最多 16 个顶层项目，更多文件可以放在一个文件夹里投放。
4. 对方自动接收到自己的 `~/Downloads`（系统显示为“下载”）。主窗口“文件传送通道”显示进度、速度、取消和失败原因，并提供“打开下载目录”。
5. 用另一台电脑的鼠标控制文件所在电脑时，也可在该电脑的通道里投放，文件会反向发回。

行为说明：

- 始终是**复制**，源文件不会被删除或移动；拖拽时不会再切屏，也不在对方桌面或任意窗口中接续原生拖拽。松手后正常移鼠标仍可切屏。
- 通道仅在连接且本机共享开启时显示。两端都需要支持协议 minor 2 的新版；旧版仍可共享键鼠，但不能用新通道传文件。
- 目标路径由接收电脑自己决定，发送端不能指定远端目录。普通文本、网页链接和没有本地文件 URL 的应用内拖拽不接受。
- 文件只经雷雳数据线传输。内容走独立 TCP 连接，输入消息不与文件内容排在同一条连接上。
- 每个文件传完做 SHA-256 校验。先写入 Downloads 里的隐藏临时目录，校验通过后原子改名；失败或取消会清理暂存文件。对方保存后返回确认，发送端才显示完成。
- 已有同名项目时另存为“名称 2”，不会覆盖。断开时若保存确认丢失，发送端会报错，但接收方可能已经保存，请重试前检查下载目录。
- 复制前检查磁盘空间；传输期间源文件被修改会报错。暂不支持断点续传。
- 保留目录结构、空文件夹、符号链接（不跟随）和修改时间；文件保留权限位，目录额外保留所有者读写执行权限。不保留扩展属性、ACL 和资源分叉。
- 成功记录保留 4 秒，失败或取消记录保留 60 秒；收到权限提示时允许访问下载目录，拒绝会显示失败。

实现与双机验收步骤见 [ADR-0002：可见文件传送通道](docs/adr/0002-visible-file-portals.md)。自动测试覆盖了回环文件传输与输入路由，但仍需在两台已签名应用上完成真实 Finder 拖放、权限及全屏场景验收。

### Fn 键的实现方式

微信输入法等应用用自己的事件监听抢在最前面截获 Fn，并且会把自己重新排回队首，所以靠事件监听没法拦住它。OpenShareMouse 在控制对方期间，在键盘驱动层把本机 Fn 临时映射为 F19（和 `hidutil` 的 `UserKeyMapping` 是同一个机制），转发时再还原成 Fn。回到本机时会立即清除映射；应用启动时也会清除上次异常退出留下的映射。已经有自定义按键映射的键盘不会被改动（这类键盘上 Fn 仍会在本机触发）。控制对方期间，Fn 组合键（如 Fn+F1 调亮度）会以 Fn+按键的形式转发给对方，不会触发本机的硬件功能。

如果发现本机 Fn 键失灵，可以在终端执行下面这条命令手动清除映射：

```sh
hidutil property --matching '{"PrimaryUsagePage":1,"PrimaryUsage":6}' --set '{"UserKeyMapping":[]}'
```

v1 不包含剪贴板共享、多台电脑、Windows。**v1 的连接没有加密，也没有配对验证**：雷雳网桥上任何运行本应用的电脑都能连上来，拖拽中的文件也以明文经过数据线。只在你自己控制的两台电脑之间使用。文件服务只接受当前已连接对方地址的请求，且每次拖拽都使用随机令牌，只能取到被拖拽的那些项目。

## 使用方法

1. 用雷雳 / USB4 数据线连接两台 Mac。
2. 两台电脑都打开“系统设置 > 网络”，确认“雷雳网桥”已启用（默认就是启用的，地址会自动分配为 169.254.x.x）。
3. 两台电脑都运行 OpenShareMouse，并在“系统设置 > 隐私与安全性 > 辅助功能”里允许它。授权后点窗口里的“重试”，或重新打开应用。
4. 窗口显示“已连接”后，在“对方电脑位于本机的”里选择左侧或右侧，两台电脑要选相反的方向。
5. 把鼠标推出那一侧的屏幕边缘即可。

## 架构

```
OpenShareMouse.app（菜单栏应用，单进程）
├─ Swift（apps/macos）：SwiftUI 界面，以及 AppKit 适配层（TransferPortal：可见文件投放区）
│     ↕ UniFFI，仅控制面（crates/ffi）
└─ Rust 静态库
   ├─ osm-model           ID、能力、几何、布局档案（报告 §5）
   ├─ osm-agent           引擎：数据线监测、对方连接、输入路由（router.rs 是纯逻辑，可单测）
   ├─ osm-core            屏幕边缘几何、按键规则，sans-IO
   ├─ osm-protocol        OSM/1 protobuf schema、封帧、发现广播
   ├─ osm-transport       雷雳网桥上的 UDP 发现与 TCP 连接（绑定 bridge0）
   ├─ osm-transfer        文件清单、流式收发、SHA-256 校验、暂存与原子提交
   └─ osm-platform-macos  雷雳检测、权限探针、event tap 捕获与注入、光标控制
```

输入热路径（event tap 回调、本机屏蔽、事件注入）放在 Rust 里，不经过 FFI。理由见 [docs/adr/0001-input-hot-path-in-rust.md](docs/adr/0001-input-hot-path-in-rust.md)。

## 构建

需要 macOS 14 以上、Xcode 16 以上，以及 Rust stable（版本由 `rust-toolchain.toml` 指定）。

```sh
make app          # 构建 Rust 和 Swift，打包并签名 build/app/OpenShareMouse.app
make run          # 构建后启动
make test         # Rust 测试 + Swift 绑定测试
make lint         # rustfmt + clippy
cargo run -p osm-agent --example status   # 在终端打印雷雳检测结果
```

`make app` 实际依次执行：

1. `scripts/build-rust.sh`：编译 `osm-ffi` 静态库，用 UniFFI 生成 Swift 绑定，打包成 `apps/macos/Frameworks/OSMCoreFFI.xcframework`。
2. `scripts/bundle-app.sh`：`swift build`，组装 `.app`，开启 Hardened Runtime 并签名。

修改 Rust 的导出接口后，要重新运行 `scripts/build-rust.sh`，Swift 端才能看到变化。

### 签名与系统权限

macOS 按代码签名记录“辅助功能”等授权。如果用 ad-hoc 签名，每次重新编译后授权都会失效。`bundle-app.sh` 会自动使用钥匙串里的第一张 `Developer ID Application` 证书，也可以用 `OSM_SIGN_IDENTITY` 指定。

运行时需要的授权：

| 授权 | 用途 |
|---|---|
| 辅助功能 | 主动 event tap：控制其他电脑时屏蔽本机输入 |
| 输入监控 | 只监听型 event tap |
| 发送事件（显示在“辅助功能”中） | 注入来自其他电脑的输入 |
| 本地网络 | 发现和连接其他设备（macOS 15 起） |

应用不使用 App Sandbox，不上架 Mac App Store，以 Developer ID 签名加公证的方式分发。

### 在 Xcode 中开发

用 Xcode 打开 `apps/macos/Package.swift` 即可编辑和调试。但需要授权的功能，必须用 `make run` 构建出的已签名 `.app` 来测试。

## 目录

```
crates/        Rust workspace
tools/         workspace 内锁定版本的 uniffi-bindgen-swift
schema/        OSM/1 wire schema（protobuf）
apps/macos/    SwiftPM 包：App、生成的绑定、Info.plist、entitlements
scripts/       构建与打包脚本
probes/        平台能力最小实验（P0）
docs/          ADR、协议合同、兼容矩阵
```

## 路线

按报告 §15.1 推进。当前处于 **P0（平台可行性）**，待验证的探针清单见 [probes/README.md](probes/README.md)。
