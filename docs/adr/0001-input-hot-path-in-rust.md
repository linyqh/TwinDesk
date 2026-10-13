# ADR-0001：输入热路径放在 Rust 平台层

状态：已采纳 · 2026-10-13

## 背景

报告 §4.2 建议 macOS 平台层用 AppKit/Objective-C/Swift 实现，§17.1 又说明 adapter 契约与语言无关。

event tap 回调必须在返回前同步完成一系列判断：是否是 Panic 热键、读取当前 ForwardPolicy 快照、决定是否屏蔽本机事件、无阻塞入队（报告 §4.3、§8.1）。回调里不能做网络、磁盘、UI 操作，也不能等锁。

## 决定

event tap 的创建、回调、屏蔽判定和 `CGEventPost` 注入，全部在 `osm-platform-macos`（Rust）中实现，直接调用 CoreGraphics C API。Swift 只负责与 AppKit 对象生命周期绑定的部分：NSPasteboard、拖放会话、覆盖层、界面。

Rust 和 Swift 之间只通过 UniFFI 传递控制面：配对、设备、布局、状态订阅、权限。

## 理由

- 回调不需要跨 FFI 回到 core 读取快照，减少了一层调用和内存所有权的复杂度。
- 避开 Swift 6 严格并发检查与 C 回调线程之间的冲突。
- Windows 端可以用同样的结构（windows-rs + 低级 hook），共用快照与队列代码。
- lan-mouse 采用相同的分层，证明可行。

## 代价

- 需要自己维护少量 CoreGraphics 的 `extern "C"` 声明。
- 需要 Swift 渲染的反馈（暗化、方向提示）要经过 UniFFI 回调，延迟高于 Swift 进程内调用，但这类反馈不在热路径上。
