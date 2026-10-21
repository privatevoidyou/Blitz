# Blitz 疾风加速
托盘常驻的小助手（Rust + Slint fluent-dark + windows-rs）：左键托盘弹出面板（软件启动器、关机/我的电脑/截图、资源占用、一键疾风），勾选要关闭的进程并自动记住。

- 配置：`%AppData%\Blitz\config.json`（若不存在，自动读取旧版 `%AppData%\OneClickBoost\config.json`）
- 开机自启：`HKCU\...\Run\Blitz`
- 构建：`cargo build --release`；测试：`cargo test`、`tests/e2e.ps1`
- 旧版 C#/WinUI 3 实现在 `legacy/`
