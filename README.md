# 一键加速小工具 (OneClickSpeedup)
C# .NET 8 + WinUI 3（unpackaged，自包含单文件）。配置保存在 `%AppData%\OneClickBoost\config.json`。
构建：`cd src/OneClickSpeedup && dotnet publish -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -p:WindowsAppSDKSelfContained=true -p:Platform=x64`
CI：`.github/workflows/build.yml`（windows-latest，产出 exe artifact + Release + 运行截图）。
