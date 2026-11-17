# 剪栈 1.0.10

剪栈是 Windows x64 剪贴板工具。Rust 实现系统剪贴板监听、托盘、快捷键、文件加密和窗口管理。Tauri 2 的 WebView2 加载本地 AK-UI CSS Core 1.1.0 和界面资源。

## 安装和使用

双击 `剪栈_1.0.10_x64-setup.exe`。安装器会请求管理员权限，将程序安装到系统盘的 `Program Files\剪栈`。安装范围默认是「安装给所有人」，也可选择「安装给本账户」。开始菜单快捷方式与登录启动入口随安装范围设置。

安装器默认勾选开机自启动、创建开始菜单快捷方式和安装完成后启动。桌面快捷方式默认为关闭。各账户仍可在剪栈设置中单独开关自己的登录自启动。

单击托盘图标或按 `Alt+V` 打开快捷收纳区，右键托盘图标可显示主界面或退出。Windows 可将图标收在托盘溢出区。主窗口的任务栏菜单提供「检索剪贴板」。

快捷收纳区底部始终显示「查看全部」和「清空全部」。剪贴板历史为空时，「清空全部」不可用。

在主界面的「剪贴板」页，`Ctrl+A` 选中所有历史记录，`Ctrl` 加左键切换单条记录的选中状态，`Shift` 加左键选择一段连续记录，也可按住左键拖动框选。按 `Delete` 或 `Backspace` 删除选中的历史记录。按 `Ctrl+F` 聚焦搜索，同时搜索历史和收藏。多个关键词用空格分隔。

在快捷收纳区、剪贴板或收藏夹点选记录，默认回到原窗口并粘贴。「设置 → 点选粘贴方式」可改为手动粘贴。使用 `Ctrl+V` 或 `Shift+Insert` 手动粘贴所选内容时，屏幕下方会显示短提示。各方式首次使用时会发送一条 Windows 通知。目标窗口不可用或 Windows 阻止输入时，可以手动粘贴已复制的内容。

在「设置 → 关于」点击「点击查看使用手册」，可在内置预览窗口阅读[图文手册](ui/manual/guide.md)。

点「预览」或在卡片上按空格打开独立预览。主窗口显示时预览以它为中心，主窗口隐藏或最小化时预览以当前屏幕的工作区为中心。再次按空格、按 `Esc` 或把焦点移出预览窗口即可关闭。多文件记录支持切换预览和在资源管理器中定位。添加内容会同时写入系统剪贴板和历史。

Windows 11 支持原生 Mica。Windows 10 选择 Mica 时回退为系统 Acrylic。关闭系统透明效果时使用主题底色。剪贴板记录支持文本、链接、图片、HTML、RTF 与资源管理器文件列表。富文本预览为只读，原始可用格式保留供粘贴。收藏文件内的文件列表保存路径，原文件仍由其所在目录管理。

## 数据位置

- 收藏默认写入 Windows 当前用户的「文档\剪栈\favorites.dpapi」。Windows DPAPI 将加密文件绑定到当前用户。使用「设置」可移动到 iCloud Drive 或其他目录。同步的是密文，跨设备或 macOS 版解密需要独立的迁移支持。
- 历史和设置存放于 `%LOCALAPPDATA%\剪栈`。历史文件的解密密钥与 Windows 登录会话绑定。退出、重启剪栈时保留历史，注销或关机后清空。
- 卸载移除开机启动入口，保留收藏文件。

## 从源码构建

安装 Rust MSVC 工具链、带 Windows SDK 的 Visual Studio C++ Build Tools，以及 Node.js。用 PowerShell 在本目录执行以下命令。Windows 主机使用 x64 WebView2 Runtime 运行应用。NSIS 安装包会在缺少运行时的设备上调用 WebView2 在线引导安装程序。

```powershell
rustup target add x86_64-pc-windows-msvc
npx --yes @tauri-apps/cli@2.12.4 build --ci --target x86_64-pc-windows-msvc --bundles nsis -- --locked
```

安装包位于 `src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis`。界面资源已在 `ui` 目录内，运行应用无需 Node.js。

AK-UI 授权随源码附在 `ui/vendor/ak-ui-MIT.txt`，也可以在应用「设置 → 关于」中打开。
