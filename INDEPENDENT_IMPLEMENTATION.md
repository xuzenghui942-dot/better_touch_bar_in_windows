# 独立实现说明

本项目的代码和界面均在新目录中重新编写。

## 参考边界

- 当前 `ThreeFiguerDrag` C# 源码只作为基础三指拖动默认值、HID 处理和桌面行为的参考。进阶能力以 `bwya77/swoosh` 最新源码提交 `7e3d1b7a8c95d02bc6dc3936f946d9f846a3fafd` 为行为基准，并在 `advanced/` 中以 Rust 设置模型、状态机和 Win32 动作实现。
- `mactouchpad` 仅用于确认 Rust、Win32 Raw Input 与 Tauri 组合的可行性，以及识别需要排除的功能范围。
- 没有复制 `mactouchpad` 的 Rust 源文件、函数实现、前端代码、图片、图标、配置文件或文字素材。
- 新图标由本项目构建脚本生成；设置界面使用本项目自行编写的 HTML、CSS 和 JavaScript。

## 隔离措施

- 新项目目录：`ThreeFingerDragRust`
- 独立应用标识：`local.threefingerdrag.rust`
- 独立设置目录、注册表值、计划任务名称和日志文件名
- 测试会拒绝界面中出现约定之外的手势和边缘控制

原项目的 C# 程序不作为构建输入，也不会由本程序加载。`ThreeFingerDragRust` 运行时只链接工作区内的 Rust crate；Swoosh 的 Apps/Appearance 字段、兼容规则和 HUD 预览均使用 Rust 实现并保存到本项目独立配置文件。

## 可复现检查

以下命令比较两个目录中所有 Rust 文件的连续 64 词元片段：

```powershell
node tools\similarity_audit.mjs . C:\Users\23879\Documents\mactouchpad 64
```

交付审计结果为 0 个相同的 64 词元连续片段。较短的 32 词元检查仅有
5 个相同片段，占新项目片段的约 0.025%，均集中在实现同一三指状态机
时不可避免的通用表达。
