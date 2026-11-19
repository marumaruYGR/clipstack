# 剪栈 1.1.2

适用于 macOS 26 及以上版本的原生剪贴板工具，通用 App 包含 Apple Silicon 和 Intel 64 位代码。AppKit 和 SwiftUI 实现 Liquid Glass 界面，Rust 管理历史、收藏索引、搜索、登录会话锁与收藏备份加密。

## 安装与使用

打开「剪栈-1.1.2.dmg」，将剪栈拖入 Applications。启动、登录自启动、重复启动和应用内重启均静默驻留菜单栏。单击图标打开快捷收纳区，右键「显示主界面」或「查看全部」打开主窗口。关闭主窗口继续运行，菜单中的「退出」结束应用。

默认快捷键为 Control+V。选取记录后默认回到原应用并发送 ⌘+V，可在设置改为手动粘贴。直接上屏需要 macOS 辅助功能授权，设置提供授权入口。应用外的粘贴按键检测受系统输入访问权限影响。点击卡片上的眼睛或按空格打开预览，失焦、Esc 或空格关闭。

设置的「关于」提供完整离线图文手册。更新内容见 [CHANGELOG.md](CHANGELOG.md)。

## 数据

历史与当前登录会话绑定，退出重开仍可找回，注销或关机后进入新的历史。「清空全部」同步清除系统剪贴板和剪栈历史，收藏继续保留。

收藏默认位于文档目录，采用 AES-256-GCM，密钥保存在本机钥匙串。移动收藏后确认重启。导出备份采用口令派生密钥，支持同格式设备互导，适合换机和重装。原库无法解密时，导入结果保存在新目录，原文件保留。

备份文件包含 8 字节标记、16 字节盐、24 字节随机数、JSON 密文及 16 字节认证标签。密钥派生为 Argon2id v1.3，64 MiB、3 次、4 路、32 字节密钥，加密为 XChaCha20-Poly1305，整个头部用作关联数据。最多 50,000 条和 512 MiB，导出口令至少 12 个 Unicode 标量，最多 1024 UTF-8 字节。原生表示额外保存在可选字段中。备份为导出时快照，文件记录保留路径。

HTML、RTF、RTFD、图像和本机文件引用保留原始表示，HTML 预览禁用脚本和外部请求。文件预览使用 Quick Look，其余文件在 Finder 定位。搜索索引仅在内存中，覆盖文字、链接、富文本提取文字及文件名。

## 更新

设置手动检查 正式发布服务，后台每日检查并发送新版通知。仅选择当前架构适用的 macOS DMG，校验大小和 SHA-256 后准备安装。当前目录可写时暂存新版，退出旧进程后替换并静默启动，保留上一个应用副本。目录不可写或从镜像运行时，在 Finder 打开已校验镜像，提示用户退出后将镜像中的剪栈拖入 Applications。

发布资产命名采用「剪栈-版本.dmg」或「clipstack-版本-macos-universal.dmg」，也可分别提供「macos-arm64」和「macos-x64」DMG。正式发布需提供 发布服务生成的 SHA-256 digest。

## 构建

使用 Xcode（完成首次启动配置）、macOS 26 或更新 SDK、Rust 1.89 或以上版本及 Cargo。支持 DEVELOPER_DIR、CARGO、RUSTC 和 SIGN_IDENTITY 环境变量。

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo test -- --test-threads=1
UNIVERSAL=1 ./build.sh ./build
./build/剪栈.app/Contents/MacOS/JianZhan --self-test
python3 -m pip install 'ds_store==1.3.1' 'mac_alias==2.2.2'
python3 package.py ./build
```

原生分层图标由 Assets/Clipstack.icon 编译。安装背景保留 1536 × 1024 像素，图标中心为 (790, 552) 和 (1320, 552)，Finder 按 768 × 512 点显示。

当前本地交付采用临时签名，公开分发需 Developer ID 签名和 Apple 公证。
