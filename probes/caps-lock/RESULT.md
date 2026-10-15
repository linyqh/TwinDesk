# P0-11：Caps Lock / 中英切换

总体结果：**实验**。单机系统接口子项通过；双机物理键盘端到端验收未完成。

日期：2026-10-15（Asia/Shanghai）

环境：macOS 27.0.1（26A434），Mac18,5，arm64。应用及探针使用 ad-hoc 签名，`codesign --verify --strict` 通过。当前钥匙串没有可用的 Developer ID Application 证书，不能据此宣称升级后的 TCC 授权可保留。

## 复现

1. 回到本机控制，避免在现有远程会话中运行探针。
2. 确保本机启用 ASCII 与非 ASCII 键盘输入源，并开启 Caps Lock 中英切换。
3. 执行 `scripts/probe-caps-lock.sh`。脚本构建并签名 `build/probes/CapsLockProbe.app`，从 bundle 中运行探针。
4. 探针临时设置键盘映射、切换输入源、切换大写锁定，随后恢复映射、原输入源和原锁定状态。

本次输出：

```text
keyboard mappings: engaged=2, observed=2, restored=true
input-source switches: true, true; elapsed: 10.0455ms
Caps Lock: original=false, toggled=true
restore input source: 0; Caps Lock restored: true
PASS: target platform operations (not a two-Mac acceptance test)
```

## 结果边界

| 子项 | 结果 | 证据与边界 |
|---|---|---|
| Fn + Caps Lock HID 映射写入、读回和恢复 | 通过 | 两个键盘服务读回匹配；没有验证真实 Caps 按键产生 F18 |
| 主队列 TIS 输入源切换 | 通过 | 连续两次切换，函数分别读回验证目标源；最后恢复原源 |
| HID 大写状态设置、读取、恢复 | 通过 | false → true → false |
| CGEvent 原生 Caps 重放切源 | 失败 | 前置单机命令行实验中，带 stateless 标志的按下/松开未改变 ABC 输入源；因此实现使用 TIS，不依赖重放 |
| 查询 Quartz flags 验证刚写入的 HID lock | 失败 | IOHID 设置后 Quartz flags 未立即反映；实现改为 IOHIDGetModifierLockState |
| Rust 测试 | 通过 | 全 workspace 109 项；最后平台层/agent 改动后相关 44 项再次通过 |
| Swift 集成测试 | 通过 | 5 项 |
| 格式、Clippy、应用构建、签名校验 | 通过 | `make lint`、`make app` |
| A 不切源、B 真实按键切源；即时输入、长按、断线、第三方输入法 | 实验 | 尚未进行双机物理键盘验收，不能从单机探针推断通过 |

完整实现策略及双机步骤见 [ADR-0003](../../docs/adr/0003-remote-caps-lock.md)。
