use tauri::{webview::PlatformWebview, WebviewWindow};
use webview2_com::{
    take_pwstr, ContextMenuRequestedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2ContextMenuItemCollection, ICoreWebView2_11,
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU,
    },
};
use windows_core::{Interface, PWSTR};

pub fn install(window: &WebviewWindow) -> tauri::Result<()> {
    window.with_webview(|view| {
        if let Err(error) = register(&view) {
            eprintln!("无法配置 WebView2 右键菜单：{error}");
        }
    })
}

fn register(view: &PlatformWebview) -> windows_core::Result<()> {
    let browser = unsafe { view.controller().CoreWebView2()? }.cast::<ICoreWebView2_11>()?;
    let handler = ContextMenuRequestedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            filter(&unsafe { args.MenuItems()? })?;
        }
        Ok(())
    }));
    // The event source owns the handler for the lifetime of the WebView.
    unsafe { browser.add_ContextMenuRequested(&handler, &mut 0) }
}

fn filter(items: &ICoreWebView2ContextMenuItemCollection) -> windows_core::Result<()> {
    unsafe {
        let mut count = 0;
        items.Count(&mut count)?;
        for index in (0..count).rev() {
            let item = items.GetValueAtIndex(index)?;
            let mut name = PWSTR::null();
            item.Name(&mut name)?;
            if matches!(
                take_pwstr(name).as_str(),
                "saveAs" | "saveImageAs" | "saveLinkAs" | "print"
            ) {
                items.RemoveValueAtIndex(index)?;
            } else {
                let mut kind = Default::default();
                item.Kind(&mut kind)?;
                if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU {
                    filter(&item.Children()?)?;
                }
            }
        }
    }
    Ok(())
}
