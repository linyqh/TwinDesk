# P0 平台探针

报告 §15.1 规定的 P0 阶段：在投入完整实现之前，先用最小程序验证受限的平台能力，把结论记成能力矩阵。每个探针要记录：系统版本、硬件、签名方式、复现步骤、结果。结果只能是“通过 / 失败 / 不支持 / 实验”之一，不要混用。

所有探针都必须在**已签名的 `.app` 内**运行（`make run`）。未签名的命令行工具拿到的 TCC 授权和本地网络授权行为不同，结论不可信。

| 编号 | 验证内容 | 对应 | 状态 |
|---|---|---|---|
| P0-01 | 主动 event tap 屏蔽本机输入；`tapDisabledByTimeout` 后自动重建；撤销授权后不会卡死系统输入 | PLAT-01, AT26 | 未开始 |
| P0-02 | 注入键鼠事件：用 `eventSourceUserData` 标记防回环；`*MouseDragged` 和双击 `clickState`；修饰键 flags | PLAT-01 | 未开始 |
| P0-03 | 源端光标冻结与隐藏：`CGAssociateMouseAndMouseCursorPosition` 加 `SetsCursorInBackground`；光标在 Dock、菜单栏、全屏应用上时的表现 | §8.3 | 未开始 |
| P0-04 | 按住 Shift 或鼠标键时 `kill -9` 目标端，检查是否卡键；检查源端光标能否恢复 | U07, §14.2 | 未开始 |
| P0-05 | Finder 文件投放到本机边缘通道，另一台 Mac 保存到 Downloads；本机鼠标与远端注入鼠标均需验证（替代原生拖拽接续，见 ADR-0002） | U01, F24 | 实验：自动测试已通过，双机验收待完成 |
| P0-06 | 剪贴板隐私：`accessBehavior`；开启 `EnablePasteboardPrivacyDeveloperPreview` 后远端粘贴的表现 | F22 | 未开始 |
| P0-07 | 本地网络权限弹窗与授权；签名变化或升级后授权是否保留 | §7.1 | 未开始 |
| P0-08 | 缩放和旋转手势合成在 macOS 27 上是否仍然有效（可能需要附带 IOHIDEvent） | F10, AT05 | 未开始 |
| P0-09 | DDC 输入源切换（IOAVService，私有接口），手头显示器逐台记录 | U06, F18 | 未开始 |
| P0-10 | Secure Input 期间的捕获与注入表现 | §17.2.2 | 未开始 |
| P0-11 | Caps Lock 中英切换：本机隔离、目标输入源切换、大写状态、断线恢复 | AT06 | 实验：逻辑测试及单机平台探针通过，双机物理键盘验收待完成；见 [记录](caps-lock/RESULT.md) |

探针代码放在本目录，每个探针一个子目录，附带 `RESULT.md`。
