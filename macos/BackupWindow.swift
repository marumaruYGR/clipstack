import AppKit
import SwiftUI
import UniformTypeIdentifiers

@MainActor final class BackupModel: ObservableObject {
    let store: Store
    let exporting: Bool
    @Published var password = ""
    @Published var confirmation = ""
    @Published var url: URL?
    @Published var busy = false
    @Published var message = ""
    @Published var reviewed: [Clip]?
    @Published var complete = false

    init(store: Store, exporting: Bool) {
        self.store = store
        self.exporting = exporting
    }

    var canSubmit: Bool {
        if reviewed != nil { return true }
        guard url != nil else { return false }
        if exporting {
            return password.unicodeScalars.count >= 12 && password.utf8.count <= 1024
                && !password.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && password == confirmation
        }
        return !password.isEmpty && password.utf8.count <= 1024
    }

    func selectFile() {
        let panel: NSSavePanel = exporting ? NSSavePanel() : NSOpenPanel()
        panel.title = exporting ? "导出收藏夹加密备份" : "导入收藏夹加密备份"
        panel.allowedContentTypes = [UTType(filenameExtension: PortableBackup.fileExtension) ?? .data]
        panel.nameFieldStringValue = "剪栈收藏备份.\(PortableBackup.fileExtension)"
        panel.canCreateDirectories = exporting
        if panel.runModal() == .OK { url = panel.url; message = "" }
    }

    func submit() {
        guard canSubmit, !busy else { return }
        if let reviewed {
            do {
                let result = try store.importFavorites(reviewed)
                message = "已导入 \(result.added) 条，跳过 \(result.skipped) 条重复收藏。"
                if let url = result.recovered { message += "\n原收藏文件已保留。恢复的收藏位于\n\(url.path)" }
                complete = true
                self.reviewed = nil
            } catch { message = error.localizedDescription }
            return
        }
        guard let url else { return }
        busy = true
        message = exporting ? "正在加密备份…" : "正在验证备份口令…"
        let secret = password
        Task {
            do {
                if exporting {
                    let clips = try store.favoritesForBackup()
                    try await Task.detached(priority: .userInitiated) {
                        try PortableBackup.export(clips, password: secret, to: url)
                    }.value
                    message = "已导出 \(clips.count) 条收藏。请分别保管备份文件和口令。"
                    complete = true
                } else {
                    reviewed = try await Task.detached(priority: .userInitiated) {
                        try PortableBackup.read(url, password: secret)
                    }.value
                    message = "口令验证通过，备份含 \(reviewed!.count) 条收藏。确认后合并，重复内容会跳过。"
                }
                password = ""
                confirmation = ""
            } catch { message = error.localizedDescription }
            busy = false
        }
    }
}

@MainActor final class BackupWindowController: NSWindowController, NSWindowDelegate {
    let model: BackupModel

    init(store: Store, exporting: Bool) {
        model = BackupModel(store: store, exporting: exporting)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 520, height: 390),
                              styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = exporting ? "导出收藏夹" : "导入收藏夹"
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
        window.contentViewController = NSHostingController(rootView: BackupView(model: model, close: { [weak self] in self?.dismiss() }))
    }

    required init?(coder: NSCoder) { fatalError("Backup windows are created in code") }

    func windowShouldClose(_ sender: NSWindow) -> Bool { !model.busy }

    func windowWillClose(_ notification: Notification) {
        model.password = ""
        model.confirmation = ""
        model.reviewed = nil
    }

    private func dismiss() {
        guard let window, !model.busy else { return }
        model.password = ""
        model.confirmation = ""
        model.reviewed = nil
        if let parent = window.sheetParent { parent.endSheet(window) }
        window.orderOut(nil)
    }
}

private struct BackupView: View {
    @ObservedObject var model: BackupModel
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label(model.exporting ? "导出收藏夹加密备份" : "从加密备份恢复收藏",
                  systemImage: model.exporting ? "lock.doc" : "lock.open")
                .font(.title3.weight(.semibold))
            if !model.complete && model.reviewed == nil {
                Text(model.exporting ? "设置至少 12 个字符的口令，并分别保管备份文件和口令。"
                     : "验证口令后再确认合并。若原收藏夹无法解密，恢复结果会保存在新目录。")
                    .font(.callout).foregroundStyle(.secondary)
                HStack {
                    Text(model.url?.lastPathComponent ?? "请选择备份文件").lineLimit(1).truncationMode(.middle)
                    Spacer()
                    Button(model.exporting ? "保存到…" : "选择文件…", action: model.selectFile)
                }
                SecureField("备份口令", text: $model.password)
                if model.exporting { SecureField("再次输入口令", text: $model.confirmation) }
            }
            if model.busy { ProgressView().controlSize(.small) }
            Text(model.message).font(.callout).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button(model.complete ? "完成" : "取消", action: close).keyboardShortcut(.cancelAction)
                if !model.complete {
                    Button(model.reviewed != nil ? "确认合并" : (model.exporting ? "导出" : "验证口令"), action: model.submit)
                        .buttonStyle(.glassProminent).keyboardShortcut(.defaultAction)
                        .disabled(!model.canSubmit)
                }
            }
        }
        .textFieldStyle(.roundedBorder)
        .padding(24)
        .frame(width: 520, height: 365)
        .disabled(model.busy)
    }
}
