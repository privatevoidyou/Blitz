# Blitz 疾风加速
托盘常驻的小助手（Rust + Slint fluent-dark + windows-rs）：左键托盘弹出面板，点击外面收起。

- 首页初始为空。点「添加快捷方式」，从开始菜单/桌面程序列表选择，或选择 `.lnk` / `.exe` 文件；只显示自己添加的项目。
- 点击快捷方式图标或文字运行。`.lnk` 的参数、工作目录交给 Windows Shell 原样处理；右键可移除首页入口，不删除原文件。失效的入口显示启动失败。
- 进程页的框、图标、文字和空白处属于同一个勾选操作，选择自动保存；右键加入白名单。
- 只结束明确选中的程序，不扩大到其他可执行文件的子进程。Windows 目录、系统/关键进程、其他会话和白名单均受保护。
- 有窗口的程序先正常退出；仍未退出的整组程序会保留，供用户保存内容或手动处理。内存整理只作用于本工具及所选程序，不遍历未选中的软件。
- 后台进程的「进程树关联」仅是可确认的父子关系提示，无法完整判断跨进程通信、共享服务等依赖；没有提示不代表可以放心关闭。

- 配置：`%AppData%\Blitz\config.json`（若不存在，自动读取旧版 `%AppData%\OneClickBoost\config.json`）
- 开机自启：`HKCU\...\Run\Blitz`
- 构建：`cargo build --release`；检查：`cargo test`、`cargo clippy --all-targets -- -D warnings`
- Windows e2e：在独立测试环境运行 `tests/e2e.ps1`，涵盖进程保护、内存整理范围、保存窗口、快捷方式参数/工作目录、自启和配置恢复。`tests/windows_fixture.rs` 只用于测试，不包含在发布 exe 中。
- 旧版 C#/WinUI 3 实现在 `legacy/`
