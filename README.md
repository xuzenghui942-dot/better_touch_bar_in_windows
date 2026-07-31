# better_touch_bar_in_windows

Windows 精确式触摸板三指拖动工具。项目使用 .NET 10、WinUI 3 和 Windows App SDK 构建。

## 项目结构

- `ThreeFingerDragOnWindows`：主 WinUI 应用
- `ThreeFingerDragElevator`：提权辅助程序
- `ThreeFingerDragOnWindows.Tests`：单元测试

## 开发

需要安装 .NET 10 SDK 和 Windows App SDK 相关开发组件。在 Windows 上执行：

```powershell
dotnet restore
dotnet build ThreeFingerDragOnWindows\ThreeFingerDragOnWindows.csproj
dotnet test ThreeFingerDragOnWindows.Tests\ThreeFingerDragOnWindows.Tests.csproj
```

本项目采用 MIT 许可证。
