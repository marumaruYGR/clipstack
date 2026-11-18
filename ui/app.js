"use strict";

const bridge = window.__TAURI__;
const invoke = (command, args) => bridge.core.invoke(command, args);
const root = document.documentElement;
const mount = document.getElementById("app");
const dialog = document.getElementById("dialog");
const dialogTitle = document.getElementById("dialog-title");
const dialogBody = document.getElementById("dialog-body");
const dialogError = document.getElementById("dialog-error");
const dialogActions = document.getElementById("dialog-actions");
const dialogClose = document.getElementById("dialog-close");
const surface = new URLSearchParams(location.search).get("window") ?? "main";
const manualPreviewId = "clipstack:user-guide";
const systemDark = matchMedia("(prefers-color-scheme: dark)");
let greetings = ["来了，里边坐。", "靠窗的位子还空着。", "字还在路上。"];
const pages = [
  ["history", "剪贴板", "clipboard"],
  ["favorites", "收藏夹", "bookmark"],
  ["settings", "设置", "settings"],
];
const itemKinds = {
  text: ["文本", "text"],
  link: ["链接", "link"],
  rich_text: ["富文本", "text"],
  image: ["图片", "image"],
  files: ["文件", "folder"],
};
const svgNamespace = "http://www.w3.org/2000/svg";
const clock = new Intl.DateTimeFormat("zh-CN", {
  hour: "2-digit", minute: "2-digit", hour12: false,
});

let state;
let page = "history";
let searchQuery = "";
let appliedQuery = "";
let searchComposing = false;
let searchTimer;
let searchRequested = false;
let previewId;
let previewSequence = 0;
let previewItem;
let pendingFavoritesImport;
let greeting = greetings[Math.floor(Math.random() * greetings.length)];
let lastCopiedId = null;
const historySelection = new Set();
let historyAnchor = null;
let historyMarquee = null;
let suppressedHistoryClick = false;
let deletingSelection = false;
let requestSequence = 0;
let noticeSequence = 0;
let noticeLeaving = false;
let maximized = false;
let transientError = "";
let dialogSession = 0;
let contentObserver;
let quickEntrance = surface === "quick";

const imageLoads = new Map();
const imageCache = new Map();
const switchChanges = new Map();
let cachedImageBytes = 0;

function node(tag, classes, value) {
  const element = document.createElement(tag);
  if (classes) element.className = classes;
  if (value != null) element.textContent = value;
  return element;
}

function icon(name) {
  const svg = document.createElementNS(svgNamespace, "svg");
  svg.setAttribute("class", "glyph");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(svgNamespace, "use");
  use.setAttribute("href", `assets/icons.svg#i-${name}`);
  svg.append(use);
  return svg;
}

function button(label, options = {}) {
  const control = node("button", options.classes);
  control.type = "button";
  if (options.icon) control.append(icon(options.icon));
  if (options.iconOnly) {
    control.setAttribute("aria-label", label);
    control.title = label;
  } else {
    control.append(node("span", "", label));
  }
  if (options.run) control.addEventListener("click", options.run);
  return control;
}

function command(label, run, glyph, style = "") {
  return button(label, { classes: `ak-button command ${style}`, icon: glyph, run });
}

function iconButton(label, glyph, run, style = "") {
  return button(label, { classes: `icon-button ${style}`, icon: glyph, iconOnly: true, run });
}

function timeStamp(timestamp) {
  const date = new Date(timestamp);
  const value = node("time", "", clock.format(date));
  value.dateTime = date.toISOString();
  value.title = date.toLocaleString("zh-CN");
  return value;
}

function press(control, active) {
  control.setAttribute("aria-pressed", String(active));
  return control;
}

function setTheme() {
  const settings = state?.settings;
  const choice = settings?.theme ?? "system";
  const dark = choice === "dark" || (choice === "system" && systemDark.matches);
  root.dataset.theme = dark ? "dark" : "light";
  root.dataset.material = settings?.material ?? "mica";
  root.dataset.fallback = String(state?.nativeBackdrop === false || !bridge);
}

function setPage(nextPage) {
  if (searchQuery || appliedQuery) {
    clearHistorySelection();
    page = nextPage;
    updateSearch("");
    return;
  }
  if (nextPage !== page) {
    clearHistorySelection();
    page = nextPage;
    render(false);
    mount.querySelector(".layout")?.classList.add("page-enter");
  }
}

function nextGreeting() {
  const choices = greetings.filter((text) => text !== greeting);
  greeting = choices[Math.floor(Math.random() * choices.length)];
}

function errorMessage(error) {
  return typeof error === "string" ? error : (error?.message ?? "操作失败，请重试。");
}

function showPageError(error) {
  transientError = errorMessage(error);
  if (surface === "preview") drawPreviewError(transientError);
  else if (state) render();
  else if (surface !== "toast") mount.replaceChildren(errorBanner(transientError));
}

async function perform(commandName, args, options = {}) {
  try {
    const result = await invoke(commandName, args);
    if (options.refresh !== false) {
      transientError = "";
      await refresh();
    }
    return result;
  } catch (error) {
    showPageError(error);
    return undefined;
  }
}

async function refresh() {
  const sequence = ++requestSequence;
  const noticeAtRequest = noticeSequence;
  const query = surface === "main" ? searchQuery.trim() : "";
  const result = await invoke("get_state", { query });
  if (sequence !== requestSequence || (surface === "main" && query !== searchQuery.trim())) return;
  const queryChanged = appliedQuery !== query;
  appliedQuery = query;
  state = result;
  const historyIds = new Set(state.history.map((record) => record.id));
  if (!historyIds.has(historyAnchor)) historyAnchor = null;
  for (const id of historySelection) {
    if (!historyIds.has(id)) historySelection.delete(id);
  }
  const retained = new Set([...historyIds, ...state.favorites.map((record) => record.id)]);
  for (const [id, source] of imageCache) {
    if (retained.has(id)) continue;
    imageCache.delete(id);
    cachedImageBytes -= source.length;
  }
  setTheme();
  if (surface === "main" || surface === "quick") {
    render(!queryChanged);
  } else if (surface === "toast" && noticeAtRequest === noticeSequence && !noticeLeaving && !mount.firstChild && state.notice) {
    drawNotice(state.notice);
  }
}

function render(preserveScroll = true) {
  if (!state || surface === "toast" || surface === "preview" || searchComposing) return;
  finishHistoryMarquee(false);
  const oldScroll = mount.querySelector("[data-scroll]");
  const top = preserveScroll ? oldScroll?.scrollTop ?? 0 : 0;
  const focused = mount.contains(document.activeElement) ? document.activeElement : null;
  const focusedKey = focused?.dataset.focusKey;
  const searchSelection = focusedKey === "search" ? [focused.selectionStart, focused.selectionEnd] : null;
  contentObserver?.disconnect();
  const content = surface === "quick" ? drawQuick() : drawMain();
  if (surface === "main" && page === "settings") animateChangedSwitches(content);
  mount.replaceChildren(content);
  const scroller = mount.querySelector("[data-scroll]");
  if (scroller) {
    scroller.scrollTop = top;
    if (surface === "quick") updateQuickSlider(scroller);
    watchImages(scroller);
  }
  if (focusedKey && !dialog.open) {
    const control = Array.from(mount.querySelectorAll("[data-focus-key]"))
      .find((element) => element.dataset.focusKey === focusedKey);
    control?.focus({ preventScroll: true });
    if (searchSelection) control?.setSelectionRange(...searchSelection);
  }
  if (quickEntrance) {
    quickEntrance = false;
    startQuickAnimation();
  }
}

function watchImages(scroller) {
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      observer.unobserve(entry.target);
      const tile = entry.target;
      imageFor(tile.dataset.image)
        .then((url) => {
          if (!tile.isConnected) return;
          const image = node("img");
          image.src = url;
          image.alt = "";
          image.draggable = false;
          tile.replaceChildren(image);
        })
        .catch(() => {});
    }
  }, { root: scroller, rootMargin: "80px" });
  contentObserver = observer;
  scroller.querySelectorAll("[data-image]").forEach((tile) => contentObserver.observe(tile));
}

async function imageFor(id) {
  if (imageCache.has(id)) return imageCache.get(id);
  if (imageLoads.has(id)) return imageLoads.get(id);
  const load = invoke("read_image", { id }).then((source) => {
    if (typeof source !== "string" || !source.startsWith("data:image/png;base64,")) {
      throw new Error("无法读取此图片。");
    }
    const maxBytes = 24 * 1024 * 1024;
    if (source.length > maxBytes) return source;
    for (const [oldId, oldSource] of imageCache) {
      if (cachedImageBytes + source.length <= maxBytes && imageCache.size < 24) break;
      imageCache.delete(oldId);
      cachedImageBytes -= oldSource.length;
    }
    imageCache.set(id, source);
    cachedImageBytes += source.length;
    return source;
  }).finally(() => imageLoads.delete(id));
  imageLoads.set(id, load);
  return load;
}

async function windowAction(action, event) {
  try {
    const origin = event ? {
      x: Math.round(event.clientX * devicePixelRatio),
      y: Math.round(event.clientY * devicePixelRatio),
    } : null;
    await invoke("window_action", { action, origin });
    if (action === "maximize") {
      maximized = await bridge.window.getCurrentWindow().isMaximized();
      render();
    }
  } catch (error) {
    showPageError(error);
  }
}

function drawTitlebar() {
  const titlebar = node("header", "titlebar");
  const drag = node("div", "title-drag");
  const mark = node("img");
  mark.src = root.dataset.theme === "dark" ? "assets/app-dark.png" : "assets/app.png";
  mark.alt = "";
  mark.draggable = false;
  drag.append(mark, node("span", "", "剪栈"));
  drag.addEventListener("mousedown", (event) => {
    if (event.button === 0 && event.detail === 1) void windowAction("drag", event);
  });
  drag.addEventListener("dblclick", () => void windowAction("maximize"));
  const controls = node("div", "window-controls");
  controls.append(
    iconButton("最小化", "minus", () => void windowAction("minimize")),
    iconButton(maximized ? "还原" : "最大化", maximized ? "restore" : "maximize", () => void windowAction("maximize")),
    iconButton("关闭到托盘", "close", () => void windowAction("close"), "close"),
  );
  titlebar.append(drag, controls);
  return titlebar;
}

function updateSearch(value) {
  clearTimeout(searchTimer);
  if (value === searchQuery && value.trim() === appliedQuery) return;
  searchQuery = value;
  clearHistorySelection();
  searchTimer = setTimeout(() => void refresh().catch(showPageError), 80);
}

function focusSearch() {
  if (dialog.open) dialog.close();
  const input = mount.querySelector(".search-input");
  searchRequested = !input;
  if (input) {
    input.focus();
    input.select();
  }
}

function drawSearch() {
  const search = node("label", "sidebar-search");
  const field = node("input", "search-input");
  field.type = "search";
  field.value = searchQuery;
  field.placeholder = "检索剪贴板";
  field.setAttribute("aria-label", "检索剪贴板和收藏夹");
  field.title = "检索剪贴板和收藏夹（Ctrl+F）";
  field.autocomplete = "off";
  field.spellcheck = false;
  field.dataset.focusKey = "search";
  field.addEventListener("compositionstart", () => { searchComposing = true; });
  field.addEventListener("compositionend", () => {
    searchComposing = false;
    updateSearch(field.value);
  });
  field.addEventListener("input", (event) => {
    if (!event.isComposing && !searchComposing) updateSearch(field.value);
  });
  field.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !event.isComposing && field.value) {
      event.preventDefault();
      field.value = "";
      updateSearch("");
    }
  });
  search.append(icon("search"), field);
  return search;
}

function drawMain() {
  const frame = node("main", "frame main");
  const layout = node("div", "layout");
  const sidebar = node("aside", "sidebar");
  const nav = node("nav", "navigation");
  nav.setAttribute("aria-label", "功能区");
  for (const [id, label, glyph] of pages) {
    const control = button(label, { icon: glyph, run: () => setPage(id) });
    control.dataset.focusKey = id;
    if (id === page && !appliedQuery) control.setAttribute("aria-current", "page");
    nav.append(control);
  }
  const quick = button("快捷收纳区", {
    classes: "rail-quick", icon: "inbox", run: () => void perform("toggle_quick", {}, { refresh: false }),
  });
  quick.dataset.focusKey = "open-quick";
  if (state.settings.shortcut) quick.append(shortcutBadge(state.settings.shortcut));
  const top = node("div", "sidebar-top");
  top.append(drawSearch(), nav);
  sidebar.append(top, quick);

  const workspace = node("section", "workspace");
  const header = node("header", "page-head");
  const heading = node("div");
  const items = page === "history" ? state.history : state.favorites;
  heading.append(node("h1", "", appliedQuery ? "搜索结果" : pages.find(([id]) => id === page)[1]));
  if (appliedQuery) heading.append(node("p", "", `${state.history.length + state.favorites.length} 项匹配`));
  else if (page === "history") heading.append(node("p", "", `${items.length} 条记录`));
  else if (page === "favorites") heading.append(node("p", "", `${items.length} 条收藏`));
  header.append(heading);
  if (page === "history" || appliedQuery) {
    const selectedCount = node("span", "", historySelectionText());
    selectedCount.dataset.selectionCount = "";
    selectedCount.setAttribute("role", "status");
    selectedCount.hidden = historySelection.size === 0;
    heading.lastElementChild.append(selectedCount);
    if (!appliedQuery) {
      const tools = node("div", "page-tools");
      const clear = command("清空全部", showClear, "trash", "warning");
      clear.dataset.focusKey = "clear";
      clear.disabled = items.length === 0;
      const add = command("添加内容", showAdd, "plus", "primary");
      add.dataset.focusKey = "add";
      tools.append(clear, add);
      header.append(tools);
    }
  }
  workspace.append(header);
  const message = transientError || state.storageError;
  if (message) workspace.append(errorBanner(message));
  const content = node("div", "workspace-content");
  content.dataset.scroll = "";
  content.append(appliedQuery ? drawSearchResults() : page === "settings" ? drawSettings() : drawRecords(items, page === "favorites"));
  if ((page === "history" || appliedQuery) && state.history.length) {
    content.classList.add("history-content");
    content.addEventListener("pointerdown", beginHistoryMarquee);
    content.addEventListener("scroll", queueHistoryMarquee, { passive: true });
  }
  workspace.append(content);
  layout.append(sidebar, workspace);
  frame.append(drawTitlebar(), layout);

  if (!maximized) {
    for (const edge of ["n", "ne", "e", "se", "s", "sw", "w", "nw"]) {
      const handle = node("div", `resize-handle resize-${edge}`);
      handle.addEventListener("mousedown", (event) => {
        if (event.button === 0) void windowAction(`resize-${edge}`, event);
      });
      frame.append(handle);
    }
  }
  return frame;
}

function errorBanner(message) {
  const banner = node("p", "page-error", message);
  banner.setAttribute("role", "alert");
  return banner;
}

function itemText(item) {
  return item.text || itemKinds[item.kind]?.[0] || "剪贴板内容";
}

function itemTitle(item) {
  return itemText(item).split(/\r?\n/u).find((line) => line.trim())?.trim() || "空白文本";
}

function itemType(item) {
  if (item.imageFile) return ["图片文件", "image"];
  if (item.kind === "files" && /\.(docx?|pdf|rtf|odt|txt|md|csv|html?)$/iu.test(itemTitle(item))) return ["文档", "text"];
  return itemKinds[item.kind] ?? itemKinds.text;
}

function drawRecords(items, fromFavorites = false) {
  if (!items.length) {
    const empty = node("section", "empty");
    empty.append(icon(page === "history" ? "inbox" : "bookmark"));
    empty.append(node("h2", "", page === "history" ? greeting : "暂无收藏"));
    return empty;
  }
  const grid = node("div", "record-grid");
  for (const item of items) grid.append(drawRecord(item, fromFavorites));
  return grid;
}

function drawSearchResults() {
  if (!(state.history.length || state.favorites.length)) {
    const empty = node("section", "empty");
    empty.append(icon("search"), node("h2", "", "没有匹配的记录"));
    return empty;
  }
  const results = node("div", "search-results");
  for (const [title, items, fromFavorites] of [["剪贴板", state.history, false], ["收藏夹", state.favorites, true]]) {
    const group = node("section", "result-group");
    group.append(node("h2", "result-heading", `${title} · ${items.length}`));
    if (items.length) group.append(drawRecords(items, fromFavorites));
    results.append(group);
  }
  return results;
}

async function copyItem(id) {
  try {
    await invoke("copy_item", { id });
    lastCopiedId = id;
    transientError = "";
    render();
  } catch (error) {
    showPageError(error);
  }
}

function historySelectionText() {
  return ` · 已选中 ${historySelection.size} 项`;
}

function updateCardSelection(card, pick) {
  const active = historySelection.has(card.dataset.historyId);
  card.dataset.selected = String(active);
  press(pick, active);
}

function updateSelectionCount() {
  const count = mount.querySelector("[data-selection-count]");
  if (count) {
    count.textContent = historySelectionText();
    count.hidden = historySelection.size === 0;
  }
}

function clearHistorySelection() {
  finishHistoryMarquee(false);
  historySelection.clear();
  historyAnchor = null;
}

function replaceHistorySelection(ids, cards = mount.querySelectorAll("[data-history-id]")) {
  if (ids.size === historySelection.size && [...ids].every((id) => historySelection.has(id))) return;
  for (const card of cards) {
    const id = card.dataset.historyId;
    if (ids.has(id) !== historySelection.has(id)) {
      if (ids.has(id)) historySelection.add(id);
      else historySelection.delete(id);
      updateCardSelection(card, card.querySelector(".record-pick"));
    }
  }
  updateSelectionCount();
}

function selectHistoryRange(id, additive) {
  const items = state.history;
  const end = items.findIndex((item) => item.id === id);
  if (end < 0) return;
  const anchor = items.findIndex((item) => item.id === historyAnchor);
  const start = anchor < 0 ? end : anchor;
  const ids = new Set(additive ? historySelection : []);
  for (let index = Math.min(start, end); index <= Math.max(start, end); index++) {
    ids.add(items[index].id);
  }
  replaceHistorySelection(ids);
  if (anchor < 0) historyAnchor = id;
}

function beginHistoryMarquee(event) {
  if (event.pointerType !== "mouse" || event.button !== 0 || event.shiftKey || historyMarquee) return;
  const scroller = event.currentTarget;
  const rect = scroller.getBoundingClientRect();
  if (event.target.closest(".record-actions") || event.clientX >= rect.left + scroller.clientWidth) return;
  historyMarquee = {
    scroller, pointerId: event.pointerId, startX: event.clientX - rect.left,
    startY: event.clientY - rect.top + scroller.scrollTop, initialX: event.clientX, initialY: event.clientY,
    x: event.clientX, y: event.clientY, additive: event.ctrlKey,
    previous: new Set(historySelection), cards: scroller.querySelectorAll("[data-history-id]"),
    box: null, frame: 0, anchor: null,
  };
}

function moveHistoryMarquee(event) {
  const drag = historyMarquee;
  if (!drag || event.pointerId !== drag.pointerId) return;
  if (!(event.buttons & 1)) {
    finishHistoryMarquee(false);
    return;
  }
  drag.x = event.clientX;
  drag.y = event.clientY;
  if (!drag.box) {
    if (Math.hypot(drag.x - drag.initialX, drag.y - drag.initialY) < 5) return;
    drag.scroller.setPointerCapture(drag.pointerId);
    drag.box = node("div", "history-marquee");
    drag.box.setAttribute("aria-hidden", "true");
    drag.scroller.append(drag.box);
  }
  event.preventDefault();
  queueHistoryMarquee();
}

function queueHistoryMarquee() {
  const drag = historyMarquee;
  if (drag?.box && !drag.frame) drag.frame = requestAnimationFrame(() => paintHistoryMarquee());
}

function paintHistoryMarquee(autoScroll = true) {
  const drag = historyMarquee;
  if (!drag?.box) return;
  drag.frame = 0;
  const { scroller } = drag;
  const { left, top } = scroller.getBoundingClientRect();
  const { clientWidth, clientHeight } = scroller;
  const y = Math.max(0, Math.min(clientHeight, drag.y - top));
  const edge = Math.min(32, clientHeight / 3);
  const speed = autoScroll ? Math.min(0, y - edge) + Math.max(0, y - clientHeight + edge) : 0;
  const oldTop = scroller.scrollTop;
  if (speed) scroller.scrollTop += Math.sign(speed) * Math.ceil(Math.abs(speed) / 2);

  const x = Math.max(0, Math.min(clientWidth, drag.x - left));
  const endY = y + scroller.scrollTop;
  const x1 = Math.min(drag.startX, x);
  const y1 = Math.min(drag.startY, endY);
  const width = Math.max(1, Math.abs(drag.startX - x));
  const height = Math.max(1, Math.abs(drag.startY - endY));
  const scrollOrigin = top - scroller.scrollTop;
  const ids = new Set(drag.additive ? drag.previous : []);
  drag.anchor = null;
  for (const card of drag.cards) {
    const rect = card.getBoundingClientRect();
    if (rect.left < left + x1 + width && rect.right > left + x1 && rect.top < scrollOrigin + y1 + height && rect.bottom > scrollOrigin + y1) {
      const id = card.dataset.historyId;
      ids.add(id);
      drag.anchor ??= id;
    }
  }
  Object.assign(drag.box.style, {
    left: `${x1}px`, top: `${y1}px`, width: `${width}px`, height: `${height}px`,
  });
  replaceHistorySelection(ids, drag.cards);
  if (scroller.scrollTop !== oldTop) queueHistoryMarquee();
}

function finishHistoryMarquee(commit) {
  const drag = historyMarquee;
  if (!drag) return;
  historyMarquee = null;
  if (drag.frame) cancelAnimationFrame(drag.frame);
  if (!drag.box) return;
  drag.box.remove();
  if (drag.scroller.hasPointerCapture(drag.pointerId)) drag.scroller.releasePointerCapture(drag.pointerId);
  suppressedHistoryClick = true;
  if (commit) {
    historyAnchor = drag.anchor;
  } else {
    const ids = new Set(state.history.filter((item) => drag.previous.has(item.id)).map((item) => item.id));
    replaceHistorySelection(ids, drag.cards);
  }
}

function handleHistoryPointerEnd(event) {
  const drag = historyMarquee;
  if (!drag || event.pointerId !== drag.pointerId) return;
  if (event.type !== "pointerup") {
    finishHistoryMarquee(false);
    return;
  }
  if (drag.frame) cancelAnimationFrame(drag.frame);
  drag.x = event.clientX;
  drag.y = event.clientY;
  paintHistoryMarquee(false);
  finishHistoryMarquee(true);
}

async function deleteSelection() {
  if (deletingSelection || !historySelection.size) return;
  deletingSelection = true;
  try {
    await perform("delete_items", { ids: [...historySelection] });
  } finally {
    deletingSelection = false;
  }
}

function handleHistoryKey(event) {
  if (!state || (page !== "history" && !appliedQuery) || dialog.open || event.defaultPrevented || event.isComposing) return;
  if (historyMarquee?.box) {
    if (event.key === "Escape") {
      event.preventDefault();
      finishHistoryMarquee(false);
    }
    return;
  }
  const target = event.target;
  if (target instanceof HTMLElement && (target.isContentEditable || target.closest("input, textarea, select"))) return;
  if (event.altKey || event.metaKey || event.shiftKey) return;
  if (event.ctrlKey && event.key.toLowerCase() === "a") {
    event.preventDefault();
    if (event.repeat) return;
    replaceHistorySelection(new Set(state.history.map((item) => item.id)));
  } else if (!event.ctrlKey && (event.key === "Delete" || event.key === "Backspace") && historySelection.size) {
    event.preventDefault();
    if (!event.repeat) void deleteSelection();
  }
}

function drawRecord(item, fromFavorites) {
  const [kind, glyph] = itemType(item);
  const card = node("article", "ak-card record");
  const focusId = fromFavorites ? `favorite-${item.id}` : item.id;
  const pick = node("button", "record-pick");
  pick.type = "button";
  const action = state.settings.pasteMode === "direct" ? "直接上屏" : "复制到系统剪贴板";
  pick.title = fromFavorites ? action : `${action}（Ctrl+单击多选，Shift+单击选择范围）`;
  pick.setAttribute("aria-label", `${action} ${itemTitle(item).slice(0, 100)}`);
  pick.dataset.focusKey = `copy-${focusId}`;
  pick.dataset.previewId = item.id;
  pick.addEventListener("click", () => {
    if (!fromFavorites) historyAnchor = item.id;
    void copyItem(item.id);
  });
  if (fromFavorites) {
    card.dataset.selected = String(lastCopiedId === item.id);
  } else {
    card.dataset.historyId = item.id;
    updateCardSelection(card, pick);
    card.addEventListener("click", (event) => {
      if ((!event.ctrlKey && !event.shiftKey) || event.button !== 0) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.shiftKey) {
        selectHistoryRange(item.id, event.ctrlKey);
        return;
      }
      if (historySelection.has(item.id)) historySelection.delete(item.id);
      else historySelection.add(item.id);
      historyAnchor = item.id;
      updateCardSelection(card, pick);
      updateSelectionCount();
    }, { capture: true });
  }
  const metadata = node("span", "record-meta");
  const label = node("span");
  label.append(icon(glyph), kind);
  metadata.append(label, timeStamp(item.createdAt));
  pick.append(metadata);
  if (item.kind === "image" || item.imageFile) {
    pick.append(node("span", "record-title", itemTitle(item)));
    const preview = node("span", "record-media");
    preview.dataset.image = item.id;
    preview.append(icon("image"));
    pick.append(preview);
  } else {
    pick.append(node("span", "record-text", itemText(item)));
  }

  const actions = node("div", "record-actions");
  const remove = button("删除", {
    classes: "delete", icon: "trash",
    run: () => fromFavorites ? showRemoveFavorite(item.id) : void perform("delete_item", { id: item.id, fromFavorites }),
  });
  remove.dataset.focusKey = `delete-${focusId}`;
  actions.append(remove);
  if (!fromFavorites) {
    const save = button(item.favorite ? "已收藏" : "收藏", {
      icon: item.favorite ? "bookmark-fill" : "bookmark",
      run: () => void perform("toggle_favorite", { id: item.id }),
    });
    save.title = item.favorite ? "取消收藏" : "收藏";
    save.dataset.focusKey = `save-${item.id}`;
    actions.append(press(save, item.favorite));
  }
  const preview = button("预览", { classes: "preview", icon: glyph, run: () => void openPreview(item.id) });
  preview.dataset.focusKey = `preview-${focusId}`;
  actions.append(preview);
  card.append(pick, actions);
  return card;
}

function drawQuick() {
  const frame = node("main", "frame quick");
  const head = node("header", "quick-head");
  const title = node("div", "quick-title");
  const heading = node("h1", "", "快捷收纳区");
  heading.tabIndex = -1;
  title.append(heading, node("p", "", `${state.history.length} 条记录`));
  const controls = node("div", "quick-controls");
  const add = command("添加内容", showAdd, "plus", "primary");
  add.dataset.focusKey = "add";
  const pin = iconButton(state.pinned ? "取消置顶" : "置顶并保持显示", "pin", () => {
    void perform("set_pinned", { pinned: !state.pinned });
  });
  pin.dataset.focusKey = "pin";
  controls.append(add, press(pin, state.pinned));
  head.append(title, controls);
  frame.append(head);
  const message = transientError || state.storageError;
  if (message) frame.append(errorBanner(message));
  if (state.history.length) {
    const body = node("div", "quick-body");
    const list = node("div", "quick-list");
    list.id = "quick-records";
    list.dataset.scroll = "";
    list.dataset.focusKey = "quick-list";
    list.tabIndex = 0;
    list.setAttribute("aria-label", "剪贴板记录");
    list.addEventListener("scroll", () => updateQuickSlider(list), { passive: true });
    for (const item of state.history) list.append(drawQuickRecord(item));
    body.append(list);
    if (state.history.length > 6) {
      body.classList.add("with-slider");
      body.append(drawQuickSlider(list));
    }
    frame.append(body);
  } else {
    const empty = node("section", "quick-empty");
    empty.append(icon("inbox"), "暂无剪贴板记录");
    frame.append(empty);
  }
  const footer = node("footer", "quick-bottom");
  const all = command("查看全部", () => void perform("open_main", {}, { refresh: false }), "window", "quiet");
  all.dataset.focusKey = "all";
  const clear = command("清空全部", showClear, "trash", "warning");
  clear.dataset.focusKey = "clear";
  clear.disabled = state.history.length === 0;
  footer.append(all, clear);
  frame.append(footer);
  return frame;
}

function updateQuickSlider(scroller) {
  const max = scroller.scrollHeight - scroller.clientHeight;
  const slider = scroller.parentElement.querySelector(".quick-slider");
  if (!slider || max <= 0) return;
  const height = slider.clientHeight;
  const thumb = Math.max(26, height * scroller.clientHeight / scroller.scrollHeight);
  const proportion = scroller.scrollTop / max;
  slider.style.setProperty("--thumb-height", `${thumb}px`);
  slider.style.setProperty("--thumb-top", `${(height - thumb) * proportion}px`);
  slider.setAttribute("aria-valuenow", String(Math.round(proportion * 100)));
}

function drawQuickSlider(scroller) {
  const slider = node("div", "quick-slider");
  const thumb = node("span");
  slider.tabIndex = 0;
  slider.dataset.focusKey = "quick-slider";
  slider.setAttribute("role", "scrollbar");
  slider.setAttribute("aria-label", "滚动剪贴板记录");
  slider.setAttribute("aria-controls", scroller.id);
  slider.setAttribute("aria-orientation", "vertical");
  slider.setAttribute("aria-valuemin", "0");
  slider.setAttribute("aria-valuemax", "100");
  slider.setAttribute("aria-valuenow", "0");
  slider.append(thumb);
  let grab = 0;
  const seek = (y) => {
    const travel = slider.clientHeight - thumb.offsetHeight;
    if (travel > 0) scroller.scrollTop =
      (y - slider.getBoundingClientRect().top - grab) / travel * (scroller.scrollHeight - scroller.clientHeight);
  };
  slider.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    slider.setPointerCapture(event.pointerId);
    grab = event.target === thumb ? event.clientY - thumb.getBoundingClientRect().top : thumb.offsetHeight / 2;
    seek(event.clientY);
  });
  slider.addEventListener("pointermove", (event) => {
    if (slider.hasPointerCapture(event.pointerId)) seek(event.clientY);
  });
  slider.addEventListener("keydown", (event) => {
    const offset = {
      ArrowUp: -53, ArrowDown: 53, PageUp: -scroller.clientHeight, PageDown: scroller.clientHeight,
      Home: -scroller.scrollHeight, End: scroller.scrollHeight,
    }[event.key];
    if (offset == null) return;
    event.preventDefault();
    scroller.scrollTop += offset;
  });
  return slider;
}

function drawQuickRecord(item) {
  const [kind, glyph] = itemType(item);
  const row = node("div", "quick-row");
  const record = node("button", "quick-copy");
  record.type = "button";
  record.title = state.settings.pasteMode === "direct" ? "直接上屏" : "复制到系统剪贴板";
  record.dataset.selected = String(lastCopiedId === item.id);
  record.dataset.focusKey = `copy-${item.id}`;
  record.dataset.previewId = item.id;
  record.addEventListener("click", () => void copyItem(item.id));
  const thumb = node("span", "quick-thumb");
  thumb.append(icon(glyph));
  if (item.kind === "image" || item.imageFile) thumb.dataset.image = item.id;
  const summary = node("span", "quick-text");
  summary.append(node("strong", "", itemTitle(item)), node("small", "", kind));
  record.append(thumb, summary, timeStamp(item.createdAt));
  const preview = iconButton("预览", "window", () => void openPreview(item.id), "quick-preview");
  preview.dataset.focusKey = `preview-${item.id}`;
  row.append(record, preview);
  return row;
}

function drawNotice(value) {
  const animate = typeof value === "object" ? value.animate : state?.settings.noticeAnimation !== false;
  value = typeof value === "object" ? value.message : value;
  if (typeof value !== "string") return;
  const notice = node("div", "ak-notice clip-notice");
  if (!animate) notice.classList.add("notice-motion-off");
  notice.setAttribute("role", "status");
  notice.append(icon("check"), node("span", "", value));
  mount.replaceChildren(notice);
}

function settingRow(title, description, glyph, control, path) {
  const row = node("section", "setting-row");
  const copy = node("div");
  const heading = node("h2", "setting-title");
  heading.append(icon(glyph), title);
  const details = node("p", "setting-description");
  details.append(description);
  copy.append(heading, details);
  if (path) {
    const storagePath = node("p", "setting-description storage-path", path);
    storagePath.title = path;
    copy.append(storagePath);
  }
  const controls = node("div", "setting-control");
  controls.append(...(Array.isArray(control) ? control : [control]));
  row.append(copy, controls);
  return row;
}

function settingPatch(patch) {
  return perform("update_settings", { patch });
}

function segment(label, key, choices) {
  const group = node("div", "segment");
  group.setAttribute("role", "group");
  group.setAttribute("aria-label", label);
  for (const [value, title] of choices) {
    const control = button(title, { run: () => void settingPatch({ [key]: value }) });
    control.dataset.focusKey = `${key}-${value}`;
    group.append(press(control, state.settings[key] === value));
  }
  return group;
}

function settingSwitch(label, key) {
  const current = state.settings[key];
  const control = button(current ? "开" : "关", {
    classes: "switch", run: () => void settingPatch({ [key]: !current }),
  });
  const track = node("span", "switch-track");
  track.setAttribute("aria-hidden", "true");
  control.prepend(track);
  control.setAttribute("role", "switch");
  control.setAttribute("aria-label", label);
  control.setAttribute("aria-checked", String(current));
  control.dataset.focusKey = key;
  return control;
}

function animateChangedSwitches(content) {
  const previous = new Map(Array.from(mount.querySelectorAll(".switch"), (control) => [control.dataset.focusKey, control]));
  const now = performance.now();
  for (const control of content.querySelectorAll(".switch")) {
    const key = control.dataset.focusKey;
    const old = previous.get(key);
    if (old && old.getAttribute("aria-checked") !== control.getAttribute("aria-checked")) {
      switchChanges.set(key, now);
    }
    const elapsed = now - switchChanges.get(key);
    if (old && elapsed < 160) {
      control.classList.add("switch-changed");
      control.style.setProperty("--switch-motion-delay", `${-elapsed}ms`);
    }
  }
}

function displayShortcut(shortcut) {
  return shortcut.split("+").map((part) => {
    const name = part.trim();
    return name.toLowerCase() === "super" ? "Win" : name;
  }).join(" + ");
}

function shortcutBadge(shortcut) {
  const name = displayShortcut(shortcut);
  const badge = node("kbd", "shortcut-badge");
  badge.setAttribute("aria-label", name);
  const keys = name.split(" + ");
  keys.forEach((key, index) => {
    badge.append(node("span", "", index < keys.length - 1 ? key + " +" : key));
  });
  return badge;
}

function drawSettings() {
  const settings = state.settings;
  const view = node("div", "settings");
  const move = command("更改位置", showMove, "folder");
  move.dataset.focusKey = "move";
  const exportBackup = command("导出", () => void showFavoritesBackup(true));
  exportBackup.dataset.focusKey = "export-favorites";
  const importBackup = command("导入", () => void showFavoritesBackup(false));
  importBackup.dataset.focusKey = "import-favorites";
  const storageDescription = document.createDocumentFragment();
  storageDescription.append(
    "收藏由当前的 Windows 操作系统加密，仅支持本地解密。如欲导出（如重装系统、迁移至其他设备），请在本机使用“导出”功能导出，并在新设备上通过“导入”按钮导入。",
    node("strong", "favorite-backup-warning", "直接复制恐将因无法解密而丢失全部数据！"),
  );
  const shortcut = command("修改快捷键", showShortcut, "keyboard");
  shortcut.dataset.focusKey = "shortcut";
  const shortcuts = settings.shortcut ? [shortcutBadge(settings.shortcut), shortcut] : [shortcut];
  view.append(
    settingRow("收藏夹位置", storageDescription, "lock", [move, exportBackup, importBackup], settings.favoritesDirectory),
    settingRow("窗口材质", "应用于主窗口、快捷收纳区和通知。", "palette",
      segment("窗口材质", "material", [["mica", "Mica"], ["acrylic", "Acrylic"]])),
    settingRow("颜色模式", "选择浅色、深色或跟随 Windows 设置。", "palette",
      segment("颜色模式", "theme", [["system", "跟随系统"], ["light", "浅色"], ["dark", "深色"]])),
    settingRow("开机自启动", "登录 Windows 后在托盘静默运行。", "power", settingSwitch("开机自启动", "autostart")),
    settingRow("系统快捷键", state.shortcutError || (settings.shortcut ? "打开或收起快捷收纳区。" : "未注册系统快捷键。"), "keyboard", shortcuts),
    settingRow("点选粘贴方式", "控制快捷收纳区、剪贴板和收藏夹中点选记录的行为。直接上屏会粘贴到原窗口，手动粘贴需回到目标窗口按 Ctrl+V。", "keyboard",
      segment("点选粘贴方式", "pasteMode", [["direct", "直接上屏"], ["manual", "手动粘贴"]])),
    settingRow("显示复制和粘贴提示", "在屏幕下方显示复制、选取和粘贴提示，约 3 秒后收起。", "bell", settingSwitch("显示复制和粘贴提示", "notifications")),
    settingRow("提示动画", "复制和粘贴提示以 0.5 秒轻缓进入、退出，可随时关闭。", "bell", settingSwitch("提示动画", "noticeAnimation")),
    settingRow("启动通知", "启动后告知剪栈已在后台运行。选择永久关闭后，后续启动也不再提示。", "bell",
      segment("启动通知", "startupNotifications", [[true, "开启"], [false, "永久关闭"]])),
    settingRow("软件更新", "每天检查一次 GitHub 正式版本，发现新版时通知。下载与安装由你决定。", "window",
      command("检查更新", () => void showUpdates())),
    drawAbout(),
  );
  return view;
}

function drawAbout() {
  const about = node("section", "about");
  const build = node("p");
  const license = node("a", "license-link", "ak-UI");
  license.href = "vendor/ak-ui-MIT.txt";
  license.draggable = false;
  license.addEventListener("click", (event) => {
    event.preventDefault();
    void showLicense();
  });
  const manual = node("a", "manual-link", "点击查看使用手册");
  manual.href = "manual/guide.md";
  manual.draggable = false;
  manual.addEventListener("click", (event) => {
    event.preventDefault();
    void showManual();
  });
  build.append("由 Rust 构建并驱动，使用 Tauri 绘制 UI，使用 ", license, " 作为主要 UI。不明白怎么用？", manual, "。");
  const portrait = node("figure", "portrait");
  const image = node("img");
  image.src = "assets/author.png";
  image.alt = "四氧化三狗";
  image.loading = "lazy";
  image.draggable = false;
  portrait.append(image, node("figcaption", "", "你认为自己是什么，那你就是什么。——摘自《狗生回忆录》"));
  about.append(
    node("h2", "", "关于"),
    node("p", "", "剪栈 1.1.1"),
    node("p", "", "剪栈是一款适用于 Mac OS 和 Windows 的剪贴板管理工具。刚复制过的内容随时能找回，反复要用的资料可以加密收藏，收藏内容不会被系统搜索索引。"),
    node("p", "", "历史记录自动收录剪栈运行时复制的文字、链接、图片、富文本和文件引用。退出剪栈后仍可找回本次登录的记录，注销或关机时清空。"),
    node("p", "", "收藏夹长期保存手动收藏的内容，例如材料模板、常用单位全称、视频导出参数和二维码。收藏使用当前 Windows 用户加密。"),
    node("p", "", "Developed by 四氧化三狗。部分代码使用 Codex 辅助开发。"),
    build,
    portrait,
  );
  return about;
}

function present(title, mode = "") {
  ++dialogSession;
  if (!dialog.open) dialog.dataset.returnFocus = document.activeElement?.dataset.focusKey ?? "";
  dialog.dataset.mode = mode;
  dialogTitle.textContent = title;
  dialogBody.replaceChildren();
  dialogActions.replaceChildren();
  dialogError.textContent = "";
  dialogError.hidden = true;
  if (!dialog.open) dialog.showModal();
  return dialogSession;
}

function dialogErrorText(message) {
  dialogError.textContent = message;
  dialogError.hidden = !message;
}

function dialogSubmit(label, work, { style = "primary", refreshAfter = true, cancel = "取消", after } = {}) {
  const session = dialogSession;
  const dismiss = command(cancel, () => dialog.close());
  const submit = command(label, async () => {
    dialogErrorText("");
    submit.disabled = true;
    dismiss.disabled = true;
    dialogClose.disabled = true;
    try {
      const result = await work();
      if (refreshAfter) await refresh().catch(showPageError);
      if (session === dialogSession && dialog.open) {
        if (after) after(result);
        else dialog.close();
      }
    } catch (error) {
      if (session === dialogSession) dialogErrorText(errorMessage(error));
    } finally {
      submit.disabled = false;
      dismiss.disabled = false;
      dialogClose.disabled = false;
    }
  }, null, style);
  dialogActions.append(dismiss, submit);
  return submit;
}

function labelField(title, control) {
  const label = node("label", "dialog-label", title);
  label.htmlFor = control.id;
  return label;
}

function showAdd() {
  present("添加内容");
  const text = node("textarea", "field");
  text.id = "add-text";
  text.placeholder = "输入要收纳的文本";
  const add = dialogSubmit("添加内容", () => invoke("add_content", { text: text.value }));
  const update = () => { add.disabled = !text.value.trim(); };
  text.addEventListener("input", update);
  text.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && event.ctrlKey && !add.disabled) {
      event.preventDefault();
      add.click();
    }
  });
  dialogBody.append(labelField("剪贴内容", text), text);
  update();
  text.focus();
}

function showClear() {
  present("清空全部", "danger");
  dialogBody.append(node("p", "dialog-description", `清空 ${state.history.length} 条历史记录及系统剪贴板？收藏仍保留。`));
  dialogSubmit("清空全部", () => invoke("clear_history"), { style: "danger" });
}

function showRemoveFavorite(id) {
  present("删除收藏", "danger");
  dialogBody.append(node("p", "dialog-description", "从收藏夹删除这条记录？"));
  dialogSubmit("删除收藏", () => invoke("delete_item", { id, fromFavorites: true }), { style: "danger" });
}

function showMove() {
  const session = present("更改收藏位置");
  dialogBody.append(node("p", "dialog-description", "选择新目录或输入 Windows 绝对路径，移动后需重启剪栈。"));
  const directory = node("input", "field");
  directory.id = "favorite-directory";
  directory.type = "text";
  directory.value = state.settings.favoritesDirectory;
  directory.spellcheck = false;
  const select = command("选择文件夹", async () => {
    select.disabled = true;
    try {
      const chosen = await invoke("choose_directory");
      if (chosen && session === dialogSession) directory.value = chosen;
      update();
    } catch (error) {
      if (session === dialogSession) dialogErrorText(errorMessage(error));
    } finally {
      select.disabled = false;
    }
  }, "folder");
  const field = node("div", "dialog-path");
  field.append(directory, select);
  dialogBody.append(labelField("目标目录", directory), field);
  const move = dialogSubmit("移动收藏文件", () => invoke("move_favorites", { directory: directory.value.trim() }), {
    after: () => showRestart(directory.value.trim()),
  });
  const update = () => {
    const value = directory.value.trim();
    move.disabled = !value || value.toLowerCase() === state.settings.favoritesDirectory?.toLowerCase();
  };
  directory.addEventListener("input", update);
  update();
  directory.focus();
  directory.select();
}

function showRestart(directory) {
  present("收藏文件已移动");
  dialogBody.append(node("p", "dialog-description", `新位置为 ${directory}。确认重启剪栈。`));
  dialogSubmit("确认重启", () => invoke("restart_app"), { refreshAfter: false, cancel: "稍后重启" });
}

async function showFavoritesBackup(saving) {
  let path;
  try {
    path = await invoke("choose_favorites_backup", { export: saving });
  } catch (error) {
    showPageError(error);
    return;
  }
  if (!path) return;

  present(saving ? "导出收藏备份" : "导入收藏备份");
  const password = backupPasswordField("backup-password");
  dialogBody.append(
    node("p", "dialog-description", path),
    node("p", "dialog-description", saving
      ? "使用至少 12 个字符的备份口令。请分别保管备份文件和口令，恢复时两者都需要。"
      : "请输入导出时设置的备份口令，验证通过后可以确认导入。"),
    labelField("备份口令", password), password,
  );

  const confirm = saving ? backupPasswordField("backup-password-confirmation") : null;
  if (confirm) dialogBody.append(labelField("确认备份口令", confirm), confirm);
  const submit = dialogSubmit(saving ? "导出加密备份" : "验证备份", async () => {
    const secret = password.value;
    if (confirm && secret !== confirm.value) throw new Error("两次输入的备份口令不一致。");
    const result = await invoke(saving ? "export_favorites" : "prepare_favorites_import", { path, password: secret });
    password.value = "";
    if (confirm) confirm.value = "";
    return result;
  }, {
    refreshAfter: false,
    after: saving ? (count) => showBackupResult("备份已导出", `已导出 ${count} 条收藏。`, path) : showImportReview,
  });
  const update = () => {
    submit.disabled = saving ? [...password.value].length < 12 || !confirm.value : !password.value;
  };
  password.addEventListener("input", update);
  confirm?.addEventListener("input", update);
  update();
  password.focus();
}

function backupPasswordField(id) {
  const field = node("input", "field");
  field.id = id;
  field.type = "password";
  field.autocomplete = "off";
  return field;
}

function showImportReview({ token, count }) {
  pendingFavoritesImport = token;
  present("确认恢复收藏");
  dialogBody.append(node("p", "dialog-description", `备份中有 ${count} 条收藏。导入时按内容去重，并与本机收藏合并。`));
  dialogSubmit("导入并合并", () => invoke("restore_favorites", { token }), {
    after: ({ added, skipped, directory }) => {
      pendingFavoritesImport = null;
      const location = directory ? `旧文件保存在原目录。恢复后的收藏位置为 ${directory}。` : "";
      showBackupResult("收藏已恢复", `已导入 ${added} 条收藏，跳过 ${skipped} 条重复记录。`, location);
    },
  });
}

function showBackupResult(title, message, path) {
  present(title);
  dialogBody.append(node("p", "dialog-description", message));
  if (path) dialogBody.append(node("p", "dialog-description", path));
  dialogActions.append(command("完成", () => dialog.close(), null, "primary"));
}

function shortcutFromKey(event) {
  const modifiers = [];
  if (event.ctrlKey) modifiers.push("Ctrl");
  if (event.altKey) modifiers.push("Alt");
  if (event.shiftKey) modifiers.push("Shift");
  if (event.metaKey) modifiers.push("Super");
  const keys = new Map([
    ["Space", "Space"], ["Enter", "Enter"], ["Tab", "Tab"], ["Backspace", "Backspace"],
    ["ArrowUp", "Up"], ["ArrowDown", "Down"], ["ArrowLeft", "Left"], ["ArrowRight", "Right"],
    ["Insert", "Insert"], ["Delete", "Delete"], ["Home", "Home"], ["End", "End"],
    ["PageUp", "PageUp"], ["PageDown", "PageDown"],
  ]);
  let key = keys.get(event.code);
  if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3);
  if (/^Digit[0-9]$/.test(event.code)) key = event.code.slice(5);
  if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(event.code)) key = event.code;
  if (key && modifiers.length > 0) return [...modifiers, key].join("+");
  return null;
}

function showShortcut() {
  present("修改系统快捷键");
  dialogBody.append(node("p", "dialog-description", "在下方按下一个组合键，或选择不注册。"));
  let shortcut = state.settings.shortcut;
  const capture = node("input", "field");
  capture.id = "shortcut-capture";
  capture.type = "text";
  capture.readOnly = true;
  capture.placeholder = "按下新快捷键";
  capture.value = shortcut ? displayShortcut(shortcut) : "";
  capture.addEventListener("mousedown", (event) => {
    event.preventDefault();
    capture.focus();
  });
  const disable = node("input");
  disable.id = "disable-shortcut";
  disable.type = "checkbox";
  disable.checked = !shortcut;
  const disableLabel = labelField("不注册系统快捷键", disable);
  disableLabel.prepend(disable, " ");
  dialogBody.append(labelField("新快捷键", capture), capture, disableLabel);
  const save = dialogSubmit("保存快捷键", () => invoke("update_settings", { patch: { shortcut: disable.checked ? null : shortcut } }));
  const update = () => { save.disabled = !disable.checked && !shortcut; };
  disable.addEventListener("change", update);
  capture.addEventListener("keydown", (event) => {
    if (event.key === "Escape" || (event.key === "Tab" && !event.ctrlKey && !event.altKey && !event.metaKey)) return;
    event.preventDefault();
    const value = shortcutFromKey(event);
    if (!value) return;
    shortcut = value;
    capture.value = displayShortcut(value);
    disable.checked = false;
    dialogErrorText("");
    update();
  });
  update();
  capture.focus();
}

async function showLicense() {
  const session = present("ak-UI 授权");
  dialogActions.append(command("关闭", () => dialog.close(), null, "primary"));
  try {
    const response = await fetch("vendor/ak-ui-MIT.txt");
    if (!response.ok) throw new Error("无法读取授权文件。");
    const license = await response.text();
    if (session === dialogSession && dialog.open) {
      dialogBody.append(node("pre", "license-text", license));
    }
  } catch (error) {
    if (session === dialogSession) dialogErrorText(errorMessage(error));
  }
}

async function showManual() {
  const session = present("使用手册");
  dialogActions.append(command("关闭", () => dialog.close(), null, "primary"));
  try {
    const item = await invoke("read_preview", { id: manualPreviewId });
    if (session === dialogSession && dialog.open) {
      dialogBody.append(renderManual(item.text, () => void perform("open_release_page", {}, { refresh: false })));
    }
  } catch (error) {
    if (session === dialogSession) dialogErrorText(errorMessage(error));
  }
}

async function showUpdates() {
  const session = present("检查更新");
  const status = node("p", "dialog-description", "正在检查更新…");
  status.setAttribute("role", "status");
  dialogBody.append(status);
  dialogActions.append(command("关闭", () => dialog.close(), null, "primary"));
  try {
    const result = await invoke("check_updates");
    if (session !== dialogSession || !dialog.open) return;
    const update = result.update;
    if (!update) {
      status.textContent = "当前使用的是最新版本 " + result.currentVersion + "。";
      return;
    }
    dialogTitle.textContent = "发现新版本";
    status.textContent = result.currentVersion + " → " + update.version + " · 下载约 " +
      (update.size / 1024 / 1024).toFixed(1) + " MB。是否更新？";
    const notes = renderManual(update.notes || "此版本未提供更新日志。", () => void perform("open_release_page", {}, { refresh: false }));
    notes.classList.add("update-notes");
    dialogBody.append(notes);
    const progress = node("p", "dialog-description");
    progress.setAttribute("role", "status");
    dialogBody.append(progress);
    dialogActions.replaceChildren();
    dialogSubmit("下载并安装", async () => {
      progress.textContent = "正在下载…";
      const unlisten = await bridge.event.listen("update-progress", ({ payload }) => {
        if (session === dialogSession) progress.textContent = "下载 " +
          Math.floor(payload.downloaded * 100 / payload.total) + "%，完成后将请求管理员权限安装。";
      });
      try {
        await invoke("install_update", { version: update.version });
      } finally {
        unlisten();
      }
    }, { refreshAfter: false, cancel: "稍后再说" });
  } catch (error) {
    if (session === dialogSession && dialog.open) {
      status.textContent = "暂时无法检查更新。";
      dialogErrorText(errorMessage(error));
    }
  }
}

async function openPreview(id) {
  if (id) await perform("open_preview", { id }, { refresh: false });
}

function handlePreviewShortcut(event) {
  if (event.defaultPrevented || event.isComposing || event.repeat || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey || event.code !== "Space") return;
  if (dialog.open || event.target.closest?.("input, textarea, select, a, [contenteditable]")) return;
  const target = event.target.closest?.("[data-preview-id]");
  const selected = historySelection.size === 1 ? historySelection.values().next().value : null;
  const hasFocusedControl = event.target.closest?.("button");
  const id = target?.dataset.previewId ?? (!hasFocusedControl ? selected : null);
  if (!id) return;
  event.preventDefault();
  void openPreview(id);
}

const richTags = new Set("section div p br hr blockquote pre code ul ol li table thead tbody tfoot tr th td h1 h2 h3 h4 h5 h6 b i strong em u s del strike sup sub span".split(" "));

function richPreview(source) {
  const template = node("template");
  template.innerHTML = source;
  const article = node("article", "preview-rich");
  const copy = (tree, target) => {
    for (const child of tree.childNodes) {
      if (child.nodeType === Node.TEXT_NODE) {
        target.append(document.createTextNode(child.textContent));
      } else if (child instanceof HTMLElement && richTags.has(child.localName)) {
        const element = node(child.localName);
        const align = child.style.textAlign;
        if (["left", "right", "center", "justify"].includes(align)) element.style.textAlign = align;
        if (child.localName === "td" || child.localName === "th") {
          element.colSpan = Math.min(100, child.colSpan);
          element.rowSpan = Math.min(100, child.rowSpan || 1);
        }
        copy(child, element);
        target.append(element);
      }
    }
  };
  copy(template.content, article);
  return article;
}

function previewAlbum(source) {
  const album = node("button", "preview-album ak-media--album ak-media--layered ak-media--split-enter");
  album.type = "button";
  album.title = "点击图片重播动画";
  album.setAttribute("aria-label", "剪贴板图片，点击重播动画");
  album.addEventListener("click", () => {
    album.classList.remove("ak-media--split-enter");
    void album.offsetWidth;
    album.classList.add("ak-media--split-enter");
  });
  for (const layer of ["back", "front"]) {
    const sheet = node("span", `ak-media__sheet ak-media__sheet--${layer}`);
    sheet.setAttribute("aria-hidden", "true");
    const image = node("img");
    image.src = source;
    image.alt = "";
    image.draggable = false;
    sheet.append(image);
    album.append(sheet);
  }
  return album;
}

function previewFrame(title) {
  const frame = node("main", "frame preview-frame");
  const header = node("header", "preview-header");
  const caption = node("div");
  caption.append(node("h1", "", "快速预览"), node("p", "", title));
  const close = iconButton("关闭预览", "close", () => void windowAction("close"));
  header.append(caption, close);
  frame.append(header);
  return frame;
}

function drawPreviewError(message) {
  const frame = previewFrame("");
  frame.append(errorBanner(message));
  mount.replaceChildren(frame);
}

function drawPreview() {
  const item = previewItem;
  const frame = previewFrame(item?.title ?? "");
  const body = node("div", `preview-content preview-${item?.kind ?? "loading"}`);
  if (!item) {
    body.append(node("p", "", "正在读取预览…"));
  } else if (item.kind === "image" && /^data:image\/(png|jpeg|gif|webp|bmp);base64,/iu.test(item.image)) {
    body.append(previewAlbum(item.image));
  } else if (item.kind === "html" && item.html) {
    body.append(richPreview(item.html));
  } else if (item.kind === "markdown" && item.text) {
    body.append(renderManual(item.text, () => void perform("open_release_page", {}, { refresh: false })));
  } else if (item.kind === "file") {
    body.append(node("p", "preview-missing", item.text ?? "可在资源管理器中查看此文件。"));
  } else {
    body.append(node("pre", "text-preview-content", item.text ?? "无法预览此记录。"));
  }
  frame.append(body);
  if (item?.files.length) {
    const footer = node("footer", "preview-footer");
    const path = node("span", "preview-path", item.files[item.index]);
    path.title = item.files[item.index];
    footer.append(path);
    if (item.files.length > 1) {
      const controls = node("div", "preview-pages");
      const previous = iconButton("上一个文件", "back", () => void loadPreview(previewId, item.index - 1));
      const next = iconButton("下一个文件", "chevron-right", () => void loadPreview(previewId, item.index + 1));
      previous.disabled = item.index === 0;
      next.disabled = item.index === item.files.length - 1;
      controls.append(previous, node("span", "", `${item.index + 1} / ${item.files.length}`), next);
      footer.append(controls);
    }
    const locate = command("在资源管理器中查看", () => {
      void invoke("locate_file", { id: previewId, index: item.index }).catch((error) => {
        frame.querySelector(".page-error")?.remove();
        frame.append(errorBanner(errorMessage(error)));
      });
    }, "folder");
    footer.append(locate);
    frame.append(footer);
  }
  mount.replaceChildren(frame);
}

async function loadPreview(id, index) {
  const sequence = ++previewSequence;
  const nextItem = id !== previewId || index == null;
  previewId = id;
  if (nextItem) {
    previewItem = null;
    drawPreview();
  }
  if (!id) return;
  try {
    const result = await invoke("read_preview", { id, index });
    if (sequence !== previewSequence) return;
    previewItem = result;
    drawPreview();
  } catch (error) {
    if (sequence === previewSequence) drawPreviewError(errorMessage(error));
  }
}


function startQuickAnimation() {
  const frame = mount.querySelector(".quick");
  if (!frame) return;
  frame.classList.remove("enter");
  void frame.offsetWidth;
  frame.classList.add("enter");
}

async function start() {
  setTheme();
  systemDark.addEventListener("change", setTheme);
  dialogClose.append(icon("close"));
  dialogClose.addEventListener("click", () => dialog.close());
  dialog.addEventListener("cancel", (event) => {
    if (dialogClose.disabled) event.preventDefault();
  });
  dialog.addEventListener("close", () => {
    for (const field of dialogBody.querySelectorAll("input[type=password]")) field.value = "";
    if (pendingFavoritesImport) {
      const token = pendingFavoritesImport;
      pendingFavoritesImport = null;
      void invoke("discard_favorites_import", { token }).catch(() => {});
    }
    const key = dialog.dataset.returnFocus;
    if (key) Array.from(mount.querySelectorAll("[data-focus-key]"))
      .find((element) => element.dataset.focusKey === key)?.focus({ preventScroll: true });
  });
  dialog.addEventListener("click", (event) => {
    if (event.target !== dialog || dialogClose.disabled) return;
    const rect = dialog.getBoundingClientRect();
    if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) dialog.close();
  });
  if (!bridge) return;

  await bridge.event.listen("state-changed", () => void refresh().catch(showPageError));
  if (surface === "main") {
    const requestSearch = async () => {
      if (await invoke("take_search_request")) focusSearch();
    };
    document.addEventListener("keydown", (event) => {
      if (event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey && event.key.toLowerCase() === "f" && !dialog.open) {
        event.preventDefault();
        focusSearch();
      }
    });
    document.addEventListener("keydown", handlePreviewShortcut);
    document.addEventListener("keydown", handleHistoryKey);
    await bridge.event.listen("search-opened", () => void requestSearch().catch(showPageError));
    window.addEventListener("pointerdown", () => { suppressedHistoryClick = false; }, { capture: true });
    window.addEventListener("pointermove", moveHistoryMarquee);
    for (const event of ["pointerup", "pointercancel", "lostpointercapture"]) {
      window.addEventListener(event, handleHistoryPointerEnd);
    }
    window.addEventListener("blur", () => finishHistoryMarquee(false));
    document.addEventListener("dragstart", (event) => {
      if (historyMarquee) event.preventDefault();
    });
    document.addEventListener("click", (event) => {
      if (event.detail && suppressedHistoryClick) {
        event.preventDefault();
        event.stopPropagation();
      }
    }, { capture: true });
    await bridge.event.listen("main-opened", () => {
      clearHistorySelection();
      page = "history";
      nextGreeting();
      if (dialog.open) dialog.close();
      if (searchQuery || appliedQuery) updateSearch("");
      else render(false);
    });
    const currentWindow = bridge.window.getCurrentWindow();
    maximized = await currentWindow.isMaximized();
    let resizeCheck;
    let resizeSequence = 0;
    window.addEventListener("resize", () => {
      const sequence = ++resizeSequence;
      clearTimeout(resizeCheck);
      resizeCheck = setTimeout(() => {
        void currentWindow.isMaximized().then((next) => {
          if (sequence === resizeSequence && next !== maximized) {
            maximized = next;
            render();
          }
        }).catch(showPageError);
      }, 90);
    });
    void fetch("assets/greetings.txt").then((response) => response.ok ? response.text() : "").then((text) => {
      const lines = text.trim().split(/\r?\n/u).filter(Boolean);
      if (!lines.length) return;
      greetings = lines;
      nextGreeting();
      if (!appliedQuery && page === "history" && state?.history.length === 0) render();
    }).catch(() => {});
    await refresh();
    await requestSearch();
    if (searchRequested) focusSearch();
    return;
  } else if (surface === "quick") {
    document.addEventListener("keydown", handlePreviewShortcut);
    await bridge.event.listen("quick-opened", () => {
      if (dialog.open) dialog.close();
      quickEntrance = true;
      render(false);
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && !dialog.open && !event.defaultPrevented) {
        void windowAction("close");
      }
    });
  } else if (surface === "preview") {
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" || (event.code === "Space" && !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey)) {
        event.preventDefault();
        void windowAction("close");
      }
    });
    await bridge.event.listen("preview-opened", ({ payload }) => void loadPreview(payload));
    const sequence = previewSequence;
    const active = await invoke("get_preview");
    if (sequence === previewSequence && active) await loadPreview(active);
  } else if (surface === "toast") {
    await bridge.event.listen("clipboard-notice", ({ payload }) => {
      ++noticeSequence;
      noticeLeaving = false;
      drawNotice(payload);
    });
    await bridge.event.listen("clipboard-notice-exit", () => {
      ++noticeSequence;
      noticeLeaving = true;
      mount.firstElementChild?.classList.add("clip-notice-leaving");
    });
  }
  await refresh();
}

void start().catch(showPageError);
