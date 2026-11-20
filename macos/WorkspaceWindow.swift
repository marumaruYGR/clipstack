import AppKit
import Combine
import SwiftUI

@MainActor final class WorkspaceSplitController: NSSplitViewController, NSToolbarDelegate {
    private let store: Store
    private let app: Controller
    private let workspaceToolbar = NSToolbar(identifier: "JianZhanWorkspace")
    private var toolbarObservation: AnyCancellable?

    private static let appName = NSToolbarItem.Identifier("app-name")
    private static let pageTitle = NSToolbarItem.Identifier("page-title")
    private static let addClip = NSToolbarItem.Identifier("add-clip")
    private static let clearHistory = NSToolbarItem.Identifier("clear-history")

    init(store: Store, app: Controller) {
        self.store = store
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("Workspace is loaded programmatically") }

    override func viewDidLoad() {
        super.viewDidLoad()
        splitView.isVertical = true
        splitView.dividerStyle = .thin

        let navigation = NSSplitViewItem(sidebarWithViewController:
            NSHostingController(rootView: WorkspaceSidebar(app: app))
        )
        navigation.canCollapse = false
        navigation.canCollapseFromWindowResize = false
        navigation.minimumThickness = 182
        navigation.maximumThickness = 238
        navigation.preferredThicknessFraction = 0.21
        addSplitViewItem(navigation)

        let detail = NSSplitViewItem(viewController:
            NSHostingController(rootView: WorkspaceContent(store: store, app: app))
        )
        detail.automaticallyAdjustsSafeAreaInsets = true
        detail.minimumThickness = 478
        addSplitViewItem(detail)
    }

    func installToolbar(in window: NSWindow) {
        workspaceToolbar.delegate = self
        workspaceToolbar.displayMode = .iconOnly
        workspaceToolbar.allowsUserCustomization = false
        workspaceToolbar.autosavesConfiguration = false
        window.toolbarStyle = .unified
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.titlebarSeparatorStyle = .none
        window.toolbar = workspaceToolbar
        toolbarObservation = app.$page.combineLatest(app.$searchText).sink { [weak self] page, query in
            let showActions = (page ?? .history) == .history && !query.contains { !$0.isWhitespace }
            self?.updateActions(show: showActions)
        }
    }

    private func updateActions(show: Bool) {
        for item in workspaceToolbar.items {
            switch item.itemIdentifier {
            case Self.addClip:
                item.isHidden = !show
            case Self.clearHistory:
                item.isHidden = !show
                item.isEnabled = true
            default:
                break
            }
        }
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [Self.appName, .sidebarTrackingSeparator, Self.pageTitle, .flexibleSpace, Self.clearHistory, Self.addClip]
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarAllowedItemIdentifiers(toolbar)
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier,
                 willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: identifier)
        switch identifier {
        case Self.appName:
            let name = NSHostingView(rootView: Text("剪栈").font(.headline).foregroundStyle(.secondary).fixedSize())
            name.setFrameSize(name.fittingSize)
            item.view = name
            item.isBordered = false
            item.autovalidates = false
            item.visibilityPriority = .high
        case Self.pageTitle:
            let title = NSHostingView(rootView: WorkspaceTitle(store: store, app: app).fixedSize())
            title.setFrameSize(title.fittingSize)
            item.view = title
            item.isBordered = false
            item.autovalidates = false
            item.visibilityPriority = .high
        case Self.addClip, Self.clearHistory:
            let clearsHistory = identifier == Self.clearHistory
            let name = clearsHistory ? "清空全部" : "添加内容"
            let button = NSHostingView(rootView: WorkspaceAction(
                clearsHistory: clearsHistory,
                action: clearsHistory ? app.clearHistory : app.addContent
            ))
            button.setFrameSize(button.fittingSize)
            item.label = name
            item.toolTip = name
            item.view = button
            item.isBordered = false
            item.autovalidates = false
            item.target = app
            item.action = clearsHistory ? #selector(Controller.clearHistory) : #selector(Controller.addContent)
            item.visibilityPriority = .high
        default:
            return nil
        }
        return item
    }

}

private struct WorkspaceAction: View {
    let clearsHistory: Bool
    let action: () -> Void

    var body: some View {
        Button(clearsHistory ? "清空全部" : "添加内容",
               systemImage: clearsHistory ? "trash" : "plus", action: action)
            .buttonStyle(.glass)
            .buttonBorderShape(.capsule)
            .controlSize(.regular)
            .padding(.horizontal, 3)
            .fixedSize()
    }
}
