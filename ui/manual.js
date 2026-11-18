"use strict";

// The bundled guide uses headings, paragraphs, lists, tables, and local images.
function renderManual(markdown, openReleasePage) {
  const article = document.createElement("article");
  article.className = "manual-document";
  const releasePage = "https://github.com/marumaruYGR/clipstack/releases";

  function appendInline(parent, text) {
    const token = /(?:\*\*([^*\n]+)\*\*|\*([^*\n]+)\*|`([^`\n]+)`|\[([^\]\n]+)\]\(([^)\n]+)\))/gu;
    let offset = 0;
    for (const part of text.matchAll(token)) {
      const [source, strong, emphasis, code, label, url] = part;
      parent.append(text.slice(offset, part.index));
      if (label && url !== releasePage) {
        parent.append(label);
      } else {
        const inline = document.createElement(strong ? "strong" : emphasis ? "em" : code ? "code" : "a");
        inline.textContent = strong ?? emphasis ?? code ?? label;
        if (label) {
          inline.href = releasePage;
          inline.addEventListener("click", (event) => {
            event.preventDefault();
            openReleasePage();
          });
        }
        parent.append(inline);
      }
      offset = part.index + source.length;
    }
    parent.append(text.slice(offset));
  }

  function textElement(tag, text) {
    const element = document.createElement(tag);
    appendInline(element, text);
    return element;
  }

  function table(lines) {
    const element = document.createElement("table");
    const header = element.createTHead();
    const body = element.createTBody();
    function appendRow(section, line, isHeader) {
      const row = section.insertRow();
      for (const cell of line.slice(1, -1).split("|")) {
        const item = textElement(isHeader ? "th" : "td", cell.trim());
        if (isHeader) item.scope = "col";
        row.append(item);
      }
    }
    appendRow(header, lines[0], true);
    for (const line of lines.slice(2)) appendRow(body, line, false);
    return element;
  }

  for (const block of markdown.trim().split(/\r?\n[ \t]*\r?\n/u)) {
    const lines = block.trim().split(/\r?\n/u);
    const heading = /^(#{1,6}) (.+)$/u.exec(lines[0]);
    const image = /^!\[([^\]]+)\]\((images\/[a-z\d-]+\.png)\)$/u.exec(lines[0]);
    if (heading) {
      article.append(textElement("h" + heading[1].length, heading[2]));
    } else if (image) {
      const picture = document.createElement("img");
      picture.alt = image[1];
      picture.src = "manual/" + image[2];
      picture.draggable = false;
      article.append(picture);
    } else if (lines.length >= 2 && lines.every((line) => line.startsWith("|") && line.endsWith("|"))) {
      article.append(table(lines));
    } else if (lines.every((line) => /^\d+\. /u.test(line)) || lines.every((line) => line.startsWith("- "))) {
      const list = document.createElement(lines[0].startsWith("- ") ? "ul" : "ol");
      for (const line of lines) list.append(textElement("li", line.replace(/^(?:\d+\.|-) /u, "")));
      article.append(list);
    } else {
      article.append(textElement("p", lines.join(" ")));
    }
  }
  return article;
}
