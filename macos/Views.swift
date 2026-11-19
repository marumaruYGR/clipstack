import AppKit
import SwiftUI

enum Page: String, CaseIterable, Identifiable {
    case history = "剪贴板", favorites = "收藏夹", settings = "设置"
    var id: Self { self }
    var icon: String {
        switch self {
        case .history: "clipboard"
        case .favorites: "star"
        case .settings: "gearshape"
        }
    }
}

struct WorkspaceSidebar: View {
    @ObservedObject var app: Controller

    var body: some View {
        VStack(spacing: 8) {
            SidebarSearchField(app: app)
                .frame(height: 32)
                .padding(.horizontal, 15)

            List(Page.allCases, selection: Binding(
                get: { app.isSearching ? nil : app.page },
                set: { if let page = $0 { app.showPage(page) } }
            )) { page in
                Label(page.rawValue, systemImage: page.icon)
                    .frame(minHeight: 34)
                    .tag(page)
            }
            .listStyle(.sidebar)
            .environment(\.defaultMinListRowHeight, 35)
            .scrollContentBackground(.hidden)

            if !app.hotkeyKeys.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("快捷收纳区").font(.caption).foregroundStyle(.secondary)
                    KeyFlow(spacing: 4) {
                        ForEach(Array(app.hotkeyKeys.enumerated()), id: \.offset) { _, key in
                            Text(key).font(.caption2.monospaced())
                                .padding(.horizontal, 6).padding(.vertical, 4)
                                .background(.quaternary, in: .rect(cornerRadius: 5))
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 15).padding(.bottom, 16)
            }
        }
        .padding(.top, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }
}

private struct SidebarSearchField: NSViewRepresentable {
    @ObservedObject var app: Controller

    func makeCoordinator() -> Coordinator { Coordinator(app: app) }

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.placeholderString = "搜索"
        field.font = .systemFont(ofSize: NSFont.systemFontSize)
        field.delegate = context.coordinator
        field.target = context.coordinator
        field.action = #selector(Coordinator.submit(_:))
        field.sendsWholeSearchString = false
        field.sendsSearchStringImmediately = true
        (field.cell as? NSSearchFieldCell)?.maximumRecents = 0
        field.setAccessibilityLabel("搜索剪贴板与收藏夹")
        field.toolTip = "搜索剪贴板与收藏夹（⌘F）"
        app.searchField = field
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        guard field.stringValue != app.searchText,
              (field.currentEditor() as? NSTextView)?.hasMarkedText() != true else { return }
        field.stringValue = app.searchText
    }

    @MainActor final class Coordinator: NSObject, NSSearchFieldDelegate {
        private weak var app: Controller?

        init(app: Controller) { self.app = app }

        func controlTextDidChange(_ notification: Notification) {
            if let field = notification.object as? NSSearchField { submit(field) }
        }

        @objc func submit(_ field: NSSearchField) {
            guard (field.currentEditor() as? NSTextView)?.hasMarkedText() != true else { return }
            app?.updateSearchText(field.stringValue)
        }
    }
}

struct WorkspaceTitle: View {
    @ObservedObject var store: Store
    @ObservedObject var app: Controller

    var body: some View {
        let page = app.page ?? .history
        HStack(alignment: .firstTextBaseline, spacing: 9) {
            Text(app.isSearching ? "搜索结果" : page.rawValue)
                .font(.title3.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
            if !app.isSearching && page != .settings {
                Text("\(page == .history ? store.history.count : store.favorites.count) 条")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
        .padding(.leading, 16)
        .frame(width: 164, alignment: .leading)
    }
}

struct WorkspaceContent: View {
    @ObservedObject var store: Store
    @ObservedObject var app: Controller

    var body: some View {
        Group {
            if !app.isSearching && app.page == .settings {
                SettingsView(app: app, store: store)
            } else if app.isSearching {
                LibrarySearchResults(results: store.search(app.searchText), store: store, app: app)
            } else {
                let favorite = app.page == .favorites
                ClipGrid(entries: favorite ? store.favorites : store.history, store: store, app: app, favorite: favorite)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { WorkspaceBackdrop().ignoresSafeArea() }
        .overlay(alignment: .top) {
            Rectangle()
                .fill(.regularMaterial)
                .mask {
                    LinearGradient(stops: [
                        .init(color: .white, location: 0),
                        .init(color: .white, location: 0.61),
                        .init(color: .clear, location: 1)
                    ], startPoint: .top, endPoint: .bottom)
                }
                .frame(height: 82)
                .ignoresSafeArea(.container, edges: .top)
                .allowsHitTesting(false)
                .accessibilityHidden(true)
        }
    }
}

private struct WorkspaceBackdrop: View {
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        ZStack {
            Color(nsColor: .windowBackgroundColor)
            LinearGradient(colors: [
                Color.accentColor.opacity(colorScheme == .dark ? 0.18 : 0.085),
                Color.accentColor.opacity(colorScheme == .dark ? 0.04 : 0.025),
                .clear
            ], startPoint: .topLeading, endPoint: .bottomTrailing)
        }
        .accessibilityHidden(true)
        .allowsHitTesting(false)
    }
}

private struct ClipGrid: View {
    let entries: [Clip]
    @ObservedObject var store: Store
    @ObservedObject var app: Controller
    let favorite: Bool

    var body: some View {
        if entries.isEmpty {
            ContentUnavailableView {
                Label(favorite ? "还没有收藏" : app.greeting, systemImage: favorite ? "star" : "clipboard")
            } description: {
                if favorite { Text("点按剪贴板卡片上的星标添加收藏。") }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                ClipCardGrid(entries: entries, store: store, app: app, favorite: favorite)
                    .padding(20)
            }
            .scrollEdgeEffectHidden(true, for: .top)
        }
    }
}

private struct LibrarySearchResults: View {
    let results: ClipSearchResults
    @ObservedObject var store: Store
    @ObservedObject var app: Controller

    var body: some View {
        if results.history.isEmpty && results.favorites.isEmpty {
            ContentUnavailableView("未找到匹配内容", systemImage: "magnifyingglass")
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 25) {
                    group("剪贴板", entries: results.history, favorite: false)
                    group("收藏夹", entries: results.favorites, favorite: true)
                }
                .padding(20)
            }
            .scrollEdgeEffectHidden(true, for: .top)
        }
    }

    @ViewBuilder private func group(_ title: String, entries: [Clip], favorite: Bool) -> some View {
        if !entries.isEmpty {
            VStack(alignment: .leading, spacing: 11) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(title).font(.headline).accessibilityAddTraits(.isHeader)
                    Text("\(entries.count) 条").font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                }
                ClipCardGrid(entries: entries, store: store, app: app, favorite: favorite)
            }
        }
    }
}

private struct ClipCardGrid: View {
    let entries: [Clip]
    @ObservedObject var store: Store
    @ObservedObject var app: Controller
    let favorite: Bool
    @State private var cardFrames: [UUID: CGRect] = [:]
    @State private var marquee: CGRect?
    @State private var initialSelection: Set<UUID> = []

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 204), spacing: 15, alignment: .top)], spacing: 15) {
            ForEach(entries) { clip in
                ClipCard(clip: clip, store: store, app: app, favorite: favorite)
                    .onGeometryChange(for: CGRect.self) { $0.frame(in: .named("cards")) } action: {
                        cardFrames[clip.id] = $0
                    }
                    .onDisappear { cardFrames[clip.id] = nil }
            }
        }
        .background(Color.clear.contentShape(Rectangle()))
        .overlay(alignment: .topLeading) {
            if let marquee {
                Rectangle().fill(.tint.opacity(0.14))
                    .overlay { Rectangle().strokeBorder(.tint) }
                    .frame(width: marquee.width, height: marquee.height)
                    .offset(x: marquee.minX, y: marquee.minY)
                    .allowsHitTesting(false)
            }
        }
        .coordinateSpace(name: "cards")
        .simultaneousGesture(DragGesture(minimumDistance: 8, coordinateSpace: .named("cards"))
            .onChanged { value in
                guard !favorite else { return }
                if marquee == nil { initialSelection = NSEvent.modifierFlags.contains(.command) ? store.selection : [] }
                let rect = CGRect(x: min(value.startLocation.x, value.location.x),
                                  y: min(value.startLocation.y, value.location.y),
                                  width: abs(value.location.x - value.startLocation.x),
                                  height: abs(value.location.y - value.startLocation.y))
                marquee = rect
                store.selection = initialSelection.union(cardFrames.filter { $0.value.intersects(rect) }.keys)
            }
            .onEnded { _ in marquee = nil })
    }
}

private struct ClipCard: View {
    let clip: Clip
    @ObservedObject var store: Store
    @ObservedObject var app: Controller
    let favorite: Bool
    @Environment(\.colorScheme) private var colorScheme
    @State private var thumbnail: NSImage?

    var body: some View {
        let selected = favorite ? app.focusedClipID == clip.id : store.selection.contains(clip.id)
        let saved = store.isFavorite(clip)
        let files = clip.files
        let cover = files.first.flatMap { $0.kind == .image ? $0 : nil }
        let outline = RoundedRectangle(cornerRadius: 19)
        VStack(alignment: .leading, spacing: 9) {
            Button {
                app.selectCard(clip, favorite: favorite)
            } label: {
                VStack(alignment: .leading, spacing: 9) {
                    HStack {
                        Label(clip.kind, systemImage: clip.symbol).foregroundStyle(.tint)
                        Spacer()
                        Text(clip.date, style: .time).foregroundStyle(.secondary)
                    }
                    .font(.caption.weight(.medium).monospacedDigit())
                    VStack(alignment: .leading, spacing: 5) {
                        if let photo = files.isEmpty ? clip.image : thumbnail {
                            Image(nsImage: photo)
                                .resizable().interpolation(.high).scaledToFit()
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                                .padding(4)
                                .background(Color.primary.opacity(colorScheme == .dark ? 0.055 : 0.035), in: .rect(cornerRadius: 11))
                                .accessibilityLabel("图片缩略图")
                            if !files.isEmpty {
                                Text(clip.text).font(.callout).lineLimit(1).truncationMode(.middle)
                            }
                        } else {
                            Text(clip.text).lineLimit(4)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                    .frame(height: 96, alignment: .topLeading)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("选取并写入剪贴板")

            HStack(spacing: 8) {
                if !favorite {
                    Button("删除", systemImage: "trash") { store.remove([clip.id]) }
                }
                Spacer(minLength: 0)
                if files.count == 1, let file = files.first, file.kind == .other {
                    Button("在 Finder 中显示", systemImage: "folder", action: file.reveal)
                        .labelStyle(.iconOnly).help("在 Finder 中显示")
                }
                Button("快速预览", systemImage: "eye") { app.showPreview(clip) }
                    .labelStyle(.iconOnly).help("快速预览（空格）")
                Button(saved ? "已收藏" : "收藏", systemImage: saved ? "star.fill" : "star") {
                    if favorite { app.confirmUnfavorite(clip) } else { store.toggleFavorite(clip) }
                }
                .foregroundStyle(saved ? Color.accentColor : Color.secondary)
            }
            .font(.caption).buttonStyle(.borderless).controlSize(.small)
        }
        .padding(13)
        .background {
            outline.fill(Color(nsColor: .controlBackgroundColor).opacity(colorScheme == .dark ? 0.94 : 0.85))
            outline.fill(selected ? Color.accentColor.opacity(colorScheme == .dark ? 0.21 : 0.10) : .clear)
        }
        .overlay {
            outline.strokeBorder(selected ? Color.accentColor.opacity(0.82) : Color.primary.opacity(0.11))
                .allowsHitTesting(false)
        }
        .accessibilityAddTraits(selected ? .isSelected : [])
        .task(id: cover?.url) {
            thumbnail = nil
            let image = await cover?.thumbnail()
            if !Task.isCancelled { thumbnail = image }
        }
    }
}

struct QuickView: View {
    @ObservedObject var store: Store
    @ObservedObject var app: Controller
    var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 9) {
                Label("快捷收纳区", systemImage: "clipboard").font(.headline).foregroundStyle(.primary)
                Spacer()
                Button("添加内容", systemImage: "plus", action: app.addContent).buttonStyle(.bordered)
                Button {
                    app.pinned.toggle()
                } label: { Image(systemName: app.pinned ? "pin.fill" : "pin") }
                .buttonStyle(.bordered)
                .foregroundStyle(.primary)
                .accessibilityLabel(app.pinned ? "取消置顶" : "置顶")
                .help(app.pinned ? "取消置顶" : "置顶")
            }
            Divider()
            if store.history.isEmpty {
                Text(app.greeting).foregroundStyle(.secondary).frame(height: 81)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(store.history) { clip in
                            QuickClipRow(clip: clip, app: app)
                        }
                    }
                }.frame(height: CGFloat(min(6, store.history.count)) * 40)
                    .scrollIndicators(.visible)
                    .scrollContentBackground(.hidden)
                    .defaultScrollAnchor(.top)
            }
            Divider()
            HStack {
                Button("查看全部", systemImage: "arrow.up.right.square", action: app.showMain).buttonStyle(.bordered)
                Spacer()
                Button("清空全部", systemImage: "trash", action: app.clearHistory)
                    .buttonStyle(.borderless)
            }
        }
        .padding(15).frame(width: 365).fixedSize(horizontal: true, vertical: true)
    }
}

private struct QuickClipRow: View {
    let clip: Clip
    @ObservedObject var app: Controller
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hovered = false

    var body: some View {
        HStack(spacing: 8) {
            Button {
                app.choose(clip)
                if !app.pinned { app.quick?.orderOut(nil) }
            } label: {
                HStack(spacing: 8) {
                    if let image = clip.image {
                        Image(nsImage: image).resizable().scaledToFit().frame(width: 24, height: 24)
                    } else {
                        Image(systemName: clip.symbol).frame(width: 24).foregroundStyle(.tint)
                    }
                    Text(clip.text).lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 4)
                    Text(clip.date, style: .time).font(.caption).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button("快速预览", systemImage: "eye") { app.showPreview(clip) }
                .labelStyle(.iconOnly).buttonStyle(.borderless).help("快速预览")
        }
        .padding(.horizontal, 5)
        .frame(height: 40)
        .background(hovered ? Color.primary.opacity(0.095) : .clear, in: .rect(cornerRadius: 9))
        .contentShape(Rectangle())
        .onHover { inside in
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.11)) { hovered = inside }
        }
    }
}

struct CopyBanner: View {
    let message: String

    var body: some View {
        HStack(spacing: 11) {
            Image(nsImage: NSApplication.shared.applicationIconImage)
                .resizable().scaledToFit()
                .frame(width: 40, height: 40)
                .accessibilityHidden(true)
            Text(message).font(.headline).foregroundStyle(.primary).fixedSize()
        }
        .frame(minWidth: 247, alignment: .leading)
        .padding(.horizontal, 13)
        .padding(.vertical, 8)
        .fixedSize()
    }
}

struct SettingsView: View {
    @ObservedObject var app: Controller
    @ObservedObject var store: Store
    var body: some View {
        Form {
            Section("通用") {
                Toggle("登录时自动启动", isOn: Binding(get: { app.login }, set: app.setLogin))
                Picker("外观", selection: $app.theme) {
                    Text("跟随系统").tag("system")
                    Text("浅色").tag("light")
                    Text("深色").tag("dark")
                }
                Toggle("显示复制和粘贴提示", isOn: $app.toasts)
                Toggle("提示动画", isOn: $app.noticeAnimation)
                Picker("启动通知", selection: $app.startupNotifications) {
                    Text("开启").tag(true)
                    Text("永久关闭").tag(false)
                }
                Picker("点选粘贴方式", selection: $app.pasteMode) {
                    ForEach(PasteMode.allCases) { Text($0.title).tag($0) }
                }
                HStack(alignment: .top) {
                    Text("直接上屏会切回原应用并尝试粘贴，需要辅助功能授权。手动粘贴使用 ⌘+V。")
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("授权直接上屏…", action: app.authorizePaste)
                }
                LabeledContent("快捷收纳区快捷键") {
                    VStack(alignment: .trailing, spacing: 7) {
                        Button(app.recordingHotkey ? "请按组合键…" : app.hotkeyLabel, action: app.recordHotkey)
                            .help("点按后直接按下组合键")
                        HStack {
                            Button("不注册", action: app.disableHotkey)
                            Button("恢复默认", action: app.restoreHotkey)
                        }
                    }
                }
                if app.recordingHotkey {
                    Text("按下新的组合键，或按 Esc 取消。")
                        .font(.callout).foregroundStyle(.secondary)
                }
                if let message = app.hotkeyMessage {
                    Label(message, systemImage: "exclamationmark.circle").font(.callout).foregroundStyle(.orange)
                }
            }
            Section("收藏夹存储") {
                LabeledContent("位置") {
                    VStack(alignment: .trailing, spacing: 6) {
                        Text(store.favoriteURL.path(percentEncoded: false))
                            .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                        Button("移动收藏夹…", action: app.moveFavorites)
                        HStack {
                            Button("导出") { app.openBackup(exporting: true) }
                                .disabled(store.favorites.isEmpty || store.favoriteProblem != nil)
                            Button("导入") { app.openBackup(exporting: false) }
                        }
                    }
                }
                if let problem = store.favoriteProblem {
                    Label(problem, systemImage: "exclamationmark.lock")
                        .font(.callout).foregroundStyle(.orange)
                }
                Text("换机或重装前可导出口令加密备份。导入时先验证口令，再确认合并，重复内容会跳过。")
                    .font(.caption).foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 6) {
                    Text("如果你希望漫游你的收藏夹，请将其移动到你的 iCloud 文件夹里")
                    Text("本软件不负担你的 iCloud 套餐费用，请合理调配存储空间安排和资费计划。")
                        .foregroundStyle(.secondary)
                }
                .font(.callout)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            }
            Section("软件更新") {
                UpdateSettingsView(service: app.updates, app: app)
            }
            Section("关于剪栈") {
                VStack(alignment: .leading, spacing: 13) {
                    if let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String {
                        Text("版本 \(version)").font(.callout).foregroundStyle(.secondary)
                    }
                    Text("剪栈是一款 macOS 原生剪贴板管理工具。刚复制过的内容随时能找回，反复要用的资料可以加密收藏，收藏的内容不会被系统检索到。")
                    aboutBullet("**历史记录** 自动收录剪栈运行时复制的文字、链接和图片，支持逐条删除和一键清空。退出剪栈后仍可找回本次登录的记录，注销或关机时清空。这里是“剪贴板的剪贴板”，你可以将需要的资料一次采撷至此，随后灵活取用。")
                    aboutBullet("**收藏夹** 用来长期保存手动收藏的内容，重启后也能随时取用。材料模板、常用单位全称、视频导出参数、经常发送的二维码，都可以收藏起来，下次直接粘贴。这些资料加密保存，Spotlight 无法检索得到。如果你希望漫游这份文档，请将其存放至 iCloud 目录。")
                    Button("点击查看使用手册", action: app.showManual).buttonStyle(.link)
                    Button("通知设置…", action: SystemNotices.openSettings).buttonStyle(.link)
                    if let url = Bundle.main.url(forResource: "About", withExtension: "png"), let image = NSImage(contentsOf: url) {
                        Image(nsImage: image).resizable().scaledToFit().frame(maxWidth: 480)
                            .accessibilityLabel("夕阳桥畔的灰手套驾驶员")
                            .frame(maxWidth: .infinity)
                    }
                    Text("你认为自己是什么，那你就是什么。——摘自《狗生回忆录》。")
                        .foregroundStyle(.secondary).frame(maxWidth: .infinity)
                }
                .textSelection(.disabled)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .scrollEdgeEffectHidden(true, for: .top)
    }

    private func aboutBullet(_ text: LocalizedStringKey) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Text("•").foregroundStyle(.secondary).accessibilityHidden(true)
            Text(text)
        }
    }
}

private struct UpdateSettingsView: View {
    @ObservedObject var service: UpdateService
    let app: Controller

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack {
                Text("版本 \(service.currentVersion)")
                Spacer()
                Button(service.checking ? "正在检查更新…" : "检查更新") { service.check() }
                    .disabled(service.checking || service.downloading)
                Button("查看更新日志", action: app.showChangelog)
            }
            Text("后台每天检查一次，有新版时发送系统通知。安装由你决定。")
                .font(.caption).foregroundStyle(.secondary)
            if !service.checking && !service.message.isEmpty { Text(service.message).font(.callout) }
            if service.downloading { ProgressView(value: service.progress) }
            if service.available != nil {
                HStack {
                    Button("新版更新日志", action: app.showUpdateNotes)
                    Button("下载并安装", action: service.downloadAndInstall)
                        .disabled(service.downloading || service.checking)
                }
            }
        }
    }
}

private struct KeyFlow: Layout {
    let spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        positions(width: proposal.width ?? 180, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let layout = positions(width: bounds.width, subviews: subviews)
        for (index, point) in layout.points.enumerated() {
            subviews[index].place(at: CGPoint(x: bounds.minX + point.x, y: bounds.minY + point.y), proposal: .unspecified)
        }
    }

    private func positions(width: CGFloat, subviews: Subviews) -> (points: [CGPoint], size: CGSize) {
        var points: [CGPoint] = []
        var x: CGFloat = 0
        var y: CGFloat = 0
        var height: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > 0 && x + size.width > width { x = 0; y += height + spacing; height = 0 }
            points.append(CGPoint(x: x, y: y))
            x += size.width + spacing
            height = max(height, size.height)
        }
        return (points, CGSize(width: width, height: y + height))
    }
}
