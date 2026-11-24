// The Host browser's page helper (design 9). It runs in its own isolated
// world in each tab's top document, so page scripts can neither see nor
// change it, and reports what the engine does not paint into the stream:
// the page's context menu, the popup of a <select>, the picker of a date or
// time input, and a new title. It also searches the page for find in page,
// and, while the Host records the tab, reports what the person does as
// steps (a value typed into a password or one-time-code field never).
//
// The Host waits on `next()`, which resolves with the messages since the
// last call. Everything here is page data: the Host caps and checks it.
(() => {
  if (globalThis.__zwiimUiHelper) return;

  const MAX_QUEUE = 20;
  const MAX_OPTIONS = 1000;
  const MAX_MATCHES = 1000;
  const IDLE_MS = 25000;
  const DATES = new Set(["date", "time", "datetime-local", "month", "week"]);
  // Where a date input's picker button sits, from its right edge.
  const PICKER_BUTTON_PX = 28;

  const queue = [];
  let wake = null;
  const post = (message) => {
    if (queue.length >= MAX_QUEUE) queue.shift();
    queue.push(message);
    if (wake) {
      const w = wake;
      wake = null;
      w();
    }
  };
  const cut = (s, n) => (typeof s === "string" && s ? s.slice(0, n) : undefined);
  const box = (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  };

  // The element waiting on a person's pick: only the latest one.
  let pending = null;
  let nextId = 0;
  const remember = (el) => {
    nextId += 1;
    pending = { id: nextId, el: new WeakRef(el) };
    return nextId;
  };

  addEventListener("contextmenu", (e) => {
    // A page that draws its own menu keeps it.
    if (e.defaultPrevented) return;
    const t = e.target instanceof Element ? e.target : null;
    const link = t && t.closest("a[href]");
    const image = t && t.closest("img");
    const editable =
      !!t &&
      (t.isContentEditable ||
        t instanceof HTMLTextAreaElement ||
        (t instanceof HTMLInputElement &&
          !["checkbox", "radio", "button", "submit", "reset", "image", "file", "range", "color"].includes(t.type)));
    post({
      kind: "menu",
      x: e.clientX,
      y: e.clientY,
      link: link ? cut(link.href, 2048) : undefined,
      image: image ? cut(image.currentSrc || image.src, 2048) : undefined,
      selection: cut(String(getSelection() || ""), 2000),
      editable,
    });
  });

  const opensPopup = (s) => !s.disabled && !s.multiple && s.size <= 1;
  const openSelect = (s) => {
    const options = [...s.options].slice(0, MAX_OPTIONS).map((o, index) => ({
      index,
      label: cut(o.label || o.text, 200) || "",
      disabled: o.disabled || (o.parentElement instanceof HTMLOptGroupElement && o.parentElement.disabled),
      selected: o.selected,
      group: o.parentElement instanceof HTMLOptGroupElement ? cut(o.parentElement.label, 200) : undefined,
    }));
    post({ kind: "select", id: remember(s), options, ...box(s) });
  };
  const openDate = (d) => {
    post({
      kind: "date",
      id: remember(d),
      input_type: d.type,
      value: d.value,
      min: cut(d.min, 64),
      max: cut(d.max, 64),
      ...box(d),
    });
  };

  // Before the page's own listeners, so the engine's popup never opens.
  addEventListener(
    "mousedown",
    (e) => {
      if (e.button !== 0 || !(e.target instanceof Element)) return;
      const s = e.target.closest("select");
      if (s && opensPopup(s)) {
        e.preventDefault();
        s.focus();
        openSelect(s);
        return;
      }
      const d = e.target;
      if (d instanceof HTMLInputElement && DATES.has(d.type) && !d.disabled && !d.readOnly) {
        if (e.clientX >= d.getBoundingClientRect().right - PICKER_BUTTON_PX) {
          e.preventDefault();
          d.focus();
          openDate(d);
        }
      }
    },
    true,
  );
  addEventListener(
    "keydown",
    (e) => {
      const s = e.target;
      if (!(s instanceof HTMLSelectElement) || !opensPopup(s)) return;
      const opens = e.key === " " || e.key === "F4" || e.key === "ArrowDown" || e.key === "ArrowUp";
      if (!opens || e.ctrlKey || e.metaKey) return;
      e.preventDefault();
      openSelect(s);
    },
    true,
  );

  // A title a script sets later: the Host reads it again.
  let lastTitle = document.title;
  new MutationObserver(() => {
    if (document.title !== lastTitle) {
      lastTitle = document.title;
      post({ kind: "title" });
    }
  }).observe(document, { subtree: true, childList: true, characterData: true });

  // Recording: each click, entry, choice and file, named the way a person
  // would name the control. Listens in the capture phase and never stops or
  // changes an event.
  let recording = false;
  const PROTECTED = new Set(["current-password", "new-password", "one-time-code"]);
  // A field that was a password once stays one when "show password" makes it text.
  const wasPassword = new WeakSet();
  addEventListener(
    "focusin",
    (e) => {
      if (e.target instanceof HTMLInputElement && e.target.type === "password") wasPassword.add(e.target);
    },
    true,
  );
  // Any autocomplete token naming a password, a one-time code or a card.
  const secretAutocomplete = (el) =>
    (el.getAttribute("autocomplete") || "")
      .toLowerCase()
      .split(/\s+/)
      .some((token) => PROTECTED.has(token) || token.startsWith("cc-"));
  const TEXT_LIKE = new Set(["", "text", "email", "tel", "url", "search", "number", "date", "time", "datetime-local", "month", "week"]);
  const squash = (s) => (typeof s === "string" ? s.replace(/\s+/g, " ").trim() : "");
  const nameOf = (el) => {
    const label = el.getAttribute("aria-label");
    if (squash(label)) return squash(label);
    const by = el.getAttribute("aria-labelledby");
    if (by) {
      const text = by
        .split(/\s+/)
        .map((id) => document.getElementById(id))
        .filter(Boolean)
        .map((n) => n.textContent)
        .join(" ");
      if (squash(text)) return squash(text);
    }
    if (el.labels && el.labels.length) {
      const text = [...el.labels].map((l) => l.textContent).join(" ");
      if (squash(text)) return squash(text);
    }
    for (const attr of ["placeholder", "title", "alt"]) {
      if (squash(el.getAttribute(attr))) return squash(el.getAttribute(attr));
    }
    if (el instanceof HTMLInputElement && ["button", "submit", "reset"].includes(el.type)) return squash(el.value);
    return squash(el.innerText || el.textContent || el.getAttribute("name") || "");
  };
  const roleOf = (el) => {
    const role = el.getAttribute("role");
    if (role) return role;
    if (el instanceof HTMLAnchorElement) return "link";
    if (el instanceof HTMLSelectElement) return "combobox";
    if (el instanceof HTMLTextAreaElement) return "textbox";
    if (el instanceof HTMLInputElement) {
      if (el.type === "password") return "password";
      if (el.type === "checkbox" || el.type === "radio") return el.type;
      if (TEXT_LIKE.has(el.type)) return "textbox";
      return "button";
    }
    return el.tagName.toLowerCase() === "summary" ? "button" : el.tagName.toLowerCase();
  };
  const recordStep = (action, el, value) =>
    post({
      kind: "step",
      step: { action, role: roleOf(el), name: cut(nameOf(el), 200) || "", value, url: location.href },
    });
  const CLICKABLE =
    "a[href],button,input[type=button],input[type=submit],input[type=reset],input[type=image]," +
    "[role=button],[role=link],[role=tab],[role=menuitem],[role=option],summary";
  addEventListener(
    "click",
    (e) => {
      if (!recording || !e.isTrusted) return;
      const t = e.target instanceof Element ? e.target.closest(CLICKABLE) : null;
      if (t) recordStep("click", t);
    },
    true,
  );
  addEventListener(
    "change",
    (e) => {
      if (!recording) return;
      const t = e.target;
      if (t instanceof HTMLInputElement && t.type === "file") {
        recordStep("file", t, cut([...(t.files || [])].map((f) => f.name).join(", "), 2000));
      } else if (t instanceof HTMLInputElement && (t.type === "checkbox" || t.type === "radio")) {
        recordStep("choose", t, t.checked ? "checked" : "unchecked");
      } else if (t instanceof HTMLSelectElement) {
        recordStep("choose", t, cut([...t.selectedOptions].map((o) => squash(o.label)).join(", "), 2000));
      } else if (t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement) {
        const protectedField =
          (t instanceof HTMLInputElement && (t.type === "password" || wasPassword.has(t))) || secretAutocomplete(t);
        if (protectedField) recordStep("protected", t);
        else recordStep("fill", t, cut(t.value, 2000));
      }
    },
    true,
  );

  // Find in page.
  let found = { query: "", ranges: [], current: -1 };
  let sheet = null;
  const highlights = typeof CSS !== "undefined" && CSS.highlights && typeof Highlight === "function";
  const paint = () => {
    if (!highlights) return;
    if (!sheet) {
      sheet = new CSSStyleSheet();
      sheet.replaceSync(
        "::highlight(zwiim-find){background-color:#fde68a;color:inherit}" +
          "::highlight(zwiim-find-current){background-color:#f59e0b;color:inherit}",
      );
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
    const current = found.ranges[found.current];
    CSS.highlights.set("zwiim-find", new Highlight(...found.ranges.filter((r) => r !== current)));
    if (current) CSS.highlights.set("zwiim-find-current", new Highlight(current));
    else CSS.highlights.delete("zwiim-find-current");
  };
  const search = (query) => {
    const ranges = [];
    const needle = query.toLowerCase();
    const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) => {
        const parent = node.parentElement;
        if (!parent || parent.closest("script,style,noscript,template")) return NodeFilter.FILTER_REJECT;
        if (parent.checkVisibility && !parent.checkVisibility()) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    for (let node = walker.nextNode(); node && ranges.length < MAX_MATCHES; node = walker.nextNode()) {
      const text = node.data.toLowerCase();
      for (let at = text.indexOf(needle); at !== -1 && ranges.length < MAX_MATCHES; at = text.indexOf(needle, at + needle.length)) {
        const range = new Range();
        range.setStart(node, at);
        range.setEnd(node, at + needle.length);
        ranges.push(range);
      }
    }
    return ranges;
  };

  globalThis.__zwiimUiHelper = {
    // Turn recording on or off in this document.
    record(on) {
      recording = !!on;
      return recording;
    },

    next() {
      if (queue.length) return Promise.resolve(JSON.stringify(queue.splice(0)));
      return new Promise((resolve) => {
        const timer = setTimeout(() => {
          wake = null;
          resolve("[]");
        }, IDLE_MS);
        wake = () => {
          clearTimeout(timer);
          resolve(JSON.stringify(queue.splice(0)));
        };
      });
    },

    // Give the waiting element what the person picked: option indexes for a
    // <select>, the value for a date input. False when it is gone.
    answer(id, value) {
      const el = pending && pending.id === id ? pending.el.deref() : null;
      pending = null;
      if (!el || !el.isConnected) return false;
      if (el instanceof HTMLSelectElement) {
        const picked = new Set(value);
        [...el.options].forEach((o, i) => {
          o.selected = picked.has(i);
        });
      } else {
        el.value = String(value);
      }
      el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    },

    // Highlight `query` on the page and move to the next match, or the one
    // before with `backwards`. An empty query clears it.
    find(query, backwards) {
      query = String(query || "").slice(0, 200);
      if (!query) {
        found = { query: "", ranges: [], current: -1 };
        if (highlights) {
          CSS.highlights.delete("zwiim-find");
          CSS.highlights.delete("zwiim-find-current");
        }
        return JSON.stringify({ matches: 0, current: 0 });
      }
      const same = query === found.query;
      const ranges = search(query);
      let current = same ? found.current + (backwards ? -1 : 1) : backwards ? ranges.length - 1 : 0;
      if (ranges.length === 0) current = -1;
      else current = (current + ranges.length) % ranges.length;
      found = { query, ranges, current };
      paint();
      const range = ranges[current];
      if (range) {
        const el = range.startContainer.parentElement;
        if (el) el.scrollIntoView({ block: "center", inline: "nearest" });
        if (!highlights) {
          const selection = getSelection();
          selection.removeAllRanges();
          selection.addRange(range);
        }
      }
      return JSON.stringify({ matches: ranges.length, current: current + 1 });
    },
  };
})();
