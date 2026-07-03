(() => {
  // Theme toggle. Initial value comes from prefers-color-scheme (set inline
  // in the template before first paint). Until the user clicks the toggle,
  // we keep following the OS preference live; after a click, their choice
  // wins for the rest of the session (no persistence).
  const root = document.documentElement;
  const themeToggle = document.getElementById('theme-toggle');
  const colorScheme = window.matchMedia ? matchMedia('(prefers-color-scheme: dark)') : null;
  let themeUserOverride = false;
  function setTheme(t) {
    root.dataset.theme = t;
    themeToggle.setAttribute('aria-label',
      t === 'dark' ? 'Switch to light theme' : 'Switch to dark theme');
  }
  setTheme(root.dataset.theme === 'dark' ? 'dark' : 'light');
  themeToggle.addEventListener('click', () => {
    themeUserOverride = true;
    setTheme(root.dataset.theme === 'dark' ? 'light' : 'dark');
  });
  if (colorScheme) {
    const onSchemeChange = (e) => { if (!themeUserOverride) setTheme(e.matches ? 'dark' : 'light'); };
    if (colorScheme.addEventListener) colorScheme.addEventListener('change', onSchemeChange);
    else colorScheme.addListener(onSchemeChange);
  }

  const list = document.getElementById('file-list');
  const sidebarTitle = document.getElementById('sidebar-title');
  const prefetchEl = document.getElementById('prefetch');
  const prefetchBar = document.getElementById('prefetch-bar');
  const prefetchLabel = document.getElementById('prefetch-label');
  const stage = document.getElementById('stage');
  const stageWrap = stage.parentElement;
  const layerBase = document.getElementById('layer-base');
  const layerHead = document.getElementById('layer-head');
  const layerEdge = document.getElementById('layer-edge');
  const divider = document.getElementById('swipe-divider');
  const placeholder = document.getElementById('placeholder');
  const modeButtons = document.querySelectorAll('.modes button');

  const BADGE_LETTERS = { added: 'A', removed: 'R', modified: 'M', unchanged: 'U' };
  const MODES = ['base', 'head', 'swipe', 'rg', 'blink'];

  let activeIdx = null;
  let mode = 'head';
  const liElements = [];

  // Background prefetch state. A queue warms every layer SVG so navigation is
  // instant, but the layer currently on screen always loads first: warming
  // pauses while the active images are still fetching (see prioritizeActiveLoad),
  // so on a slow link the open page is never left blank behind the batch.
  const PREFETCH_BAR_MIN = 8;     // hide the progress chrome for trivially small diffs
  const PREFETCH_CONCURRENCY = 4; // background fetches in flight; low keeps the active view responsive
  const prefetched = new Map();   // url -> Image, held so the browser keeps each cached/decoded
  let prefetchQueue = [];
  let prefetchInFlight = 0;
  let prefetchPaused = false;
  let prefetchTotal = 0;
  let prefetchDone = 0;
  let prefetchShowBar = false;
  let activeLoadGen = 0;          // bumps each selection so stale load callbacks can't resume warming

  // Pan/zoom state. Persists across file switches; reset with `0`.
  let scale = 1;
  let tx = 0;
  let ty = 0;
  const MIN_SCALE = 0.1;
  const MAX_SCALE = 80;

  function applyTransform() {
    stage.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
  }

  function resetView() {
    scale = 1;
    tx = 0;
    ty = 0;
    applyTransform();
  }

  // Scale the current page to fit the viewport, centered.
  function fitView() {
    const stageW = stage.offsetWidth;
    const stageH = stage.offsetHeight;
    const wrapW = stageWrap.clientWidth;
    const wrapH = stageWrap.clientHeight;
    if (!stageW || !stageH || !wrapW || !wrapH) return;
    const k = Math.min(wrapW / stageW, wrapH / stageH) * 0.96;
    scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, k));
    tx = (wrapW - stageW * scale) / 2;
    ty = (wrapH - stageH * scale) / 2;
    applyTransform();
  }

  // Blink mode alternates base/head by toggling a class on the stage.
  let blinkTimer = null;
  function stopBlink() {
    if (blinkTimer) { clearInterval(blinkTimer); blinkTimer = null; }
    stage.classList.remove('blink-head');
  }
  function startBlink() {
    stopBlink();
    let showHead = false;
    blinkTimer = setInterval(() => {
      showHead = !showHead;
      stage.classList.toggle('blink-head', showHead);
    }, 500);
  }

  function setMode(m) {
    if (!MODES.includes(m)) return;
    mode = m;
    stage.dataset.mode = m;
    stageWrap.classList.toggle('swipe-cursor', m === 'swipe');
    modeButtons.forEach((b) => b.classList.toggle('active', b.dataset.mode === m));
    stopBlink();
    if (m === 'blink') startBlink();
  }

  // Change currently active layer.
  function flipLayer() {
    setMode(mode === 'base' ? 'head' : 'base');
  }

  function applyAvailability(entry) {
    const hasBase = entry.status !== 'added';
    const hasHead = entry.status !== 'removed';

    layerBase.src = hasBase ? `a/svg/${entry.path}` : '';
    layerHead.src = hasHead ? `b/svg/${entry.path}` : '';

    // Board-outline context for PCB layers (already an index-relative URL).
    if (entry.edge) {
      layerEdge.src = entry.edge;
      stage.classList.add('has-edge');
    } else {
      layerEdge.src = '';
      stage.classList.remove('has-edge');
    }

    // Force base layer to drive container size when present; otherwise let head do it.
    if (hasBase) {
      layerBase.style.visibility = '';
      layerHead.style.position = 'absolute';
    } else {
      layerBase.style.visibility = 'hidden';
      layerHead.style.position = 'static';
      layerHead.style.width = '';
      layerHead.style.height = '';
    }

    // If a side is missing, only certain modes make sense — fall back to whichever exists.
    if (!hasBase) setMode('head');
    else if (!hasHead) setMode('base');
    else setMode(mode);

    // Disable buttons for missing sides
    modeButtons.forEach((b) => {
      const m = b.dataset.mode;
      const needsBoth = m === 'swipe' || m === 'rg' || m === 'blink';
      b.disabled = (needsBoth && (!hasBase || !hasHead)) ||
                   (m === 'base' && !hasBase) ||
                   (m === 'head' && !hasHead);
    });

    // The just-shown layer takes priority over background warming.
    prioritizeActiveLoad();
  }

  function select(idx) {
    if (activeIdx !== null) liElements[activeIdx].classList.remove('active');
    activeIdx = idx;
    liElements[idx].classList.add('active');
    const entry = entries[idx];
    applyAvailability(entry);
  }

  // Swipe interaction: move cursor over stage to drag the divider.
  // Divider position is computed in stage's own coordinate space so it stays
  // aligned regardless of zoom/pan.
  stageWrap.addEventListener('mousemove', (e) => {
    if (mode !== 'swipe') return;
    const rect = stageWrap.getBoundingClientRect();
    const wrapX = e.clientX - rect.left;
    const stageX = (wrapX - tx) / scale;
    const stageW = stage.offsetWidth;
    const clamped = Math.max(0, Math.min(stageW, stageX));
    const pct = (clamped / stageW) * 100;
    stage.style.setProperty('--swipe', `${pct}%`);
    divider.style.left = `${clamped}px`;
  });

  // Wheel zoom — anchored on cursor position. Scale exponentially by the
  // actual scroll delta (normalized across px/line/page wheel units and
  // clamped per event) so a trackpad two-finger flick, which fires many small
  // events, zooms at a similar rate to a mouse wheel instead of rocketing.
  stageWrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    const rect = stageWrap.getBoundingClientRect();
    const cx = e.clientX - rect.left;
    const cy = e.clientY - rect.top;
    let dy = e.deltaY;
    if (e.deltaMode === 1) dy *= 16;          // lines -> approx px
    else if (e.deltaMode === 2) dy *= rect.height; // pages -> px
    dy = Math.max(-100, Math.min(100, dy));   // tame momentum spikes
    const factor = Math.exp(-dy * 0.0015);
    const newScale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, scale * factor));
    if (newScale === scale) return;
    tx = cx - ((cx - tx) / scale) * newScale;
    ty = cy - ((cy - ty) / scale) * newScale;
    scale = newScale;
    applyTransform();
  }, { passive: false });

  // Drag to pan (disabled while in swipe mode, since the cursor drives the wipe).
  let dragging = false;
  let dragOrigin = null;
  stageWrap.addEventListener('mousedown', (e) => {
    if (mode === 'swipe') return;
    if (e.button !== 0) return;
    dragging = true;
    dragOrigin = { x: e.clientX - tx, y: e.clientY - ty };
    stageWrap.classList.add('grabbing');
  });
  document.addEventListener('mousemove', (e) => {
    if (!dragging) return;
    tx = e.clientX - dragOrigin.x;
    ty = e.clientY - dragOrigin.y;
    applyTransform();
  });
  document.addEventListener('mouseup', () => {
    dragging = false;
    stageWrap.classList.remove('grabbing');
  });

  // Double-click anywhere on the stage fits the page to the viewport.
  stageWrap.addEventListener('dblclick', () => fitView());

  // Mode buttons
  modeButtons.forEach((b) => {
    b.addEventListener('click', () => {
      if (!b.disabled) setMode(b.dataset.mode);
    });
  });

  // Keyboard shortcuts
  document.addEventListener('keydown', (e) => {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
    const map = { '1': 'base', '2': 'head', '3': 'swipe', '4': 'rg', '5': 'blink' };
    if (map[e.key]) {
      const btn = [...modeButtons].find((b) => b.dataset.mode === map[e.key]);
      if (btn && !btn.disabled) setMode(map[e.key]);
    } else if (e.key === ' ') {
      e.preventDefault();
      flipLayer();
    } else if (e.key === '0') {
      resetView();
    } else if (e.key === 'f') {
      fitView();
    } else if (e.key === 'ArrowDown' || e.key === 'j') {
      if (activeIdx !== null && activeIdx + 1 < entries.length) select(activeIdx + 1);
    } else if (e.key === 'ArrowUp' || e.key === 'k') {
      if (activeIdx !== null && activeIdx > 0) select(activeIdx - 1);
    }
  });

  if (entries.length === 0) {
    document.querySelector('main').innerHTML =
      '<div class="nothing">No visual changes between the selected revisions.</div>';
    return;
  }

  const KIND_LABELS = { sch: 'Schematics', pcb: 'PCB layers' };
  let currentProject = null;
  let currentKind = null;
  entries.forEach((e, i) => {
    // Group by project, then by kind within each project.
    if (e.project !== currentProject) {
      const ph = document.createElement('li');
      ph.className = 'project-header';
      ph.textContent = e.project;
      list.appendChild(ph);
      currentProject = e.project;
      currentKind = null;
    }
    if (e.kind !== currentKind) {
      const header = document.createElement('li');
      header.className = 'group-header';
      header.textContent = KIND_LABELS[e.kind] || e.kind;
      list.appendChild(header);
      currentKind = e.kind;
    }
    const li = document.createElement('li');
    const badge = document.createElement('span');
    badge.className = 'badge ' + e.status;
    badge.textContent = BADGE_LETTERS[e.status] || '?';
    const name = document.createElement('span');
    name.className = 'path';
    name.textContent = e.name;
    li.appendChild(badge);
    li.appendChild(name);
    li.onclick = () => select(i);
    list.appendChild(li);
    liElements[i] = li;
  });
  select(0);

  // Warm an in-memory cache of every layer SVG so switching pages - between
  // layers, and between PCB and schematic views - is instant. The viewer reuses
  // three <img> elements and swaps their `src`, so once you switch away the old
  // SVG is unreferenced and the browser may drop its decoded bitmap. The report
  // is a static page opened off disk (file://) with no HTTP cache to fall back
  // on, so revisiting a page re-reads and re-decodes from scratch. Holding a
  // reference to each Image keeps it alive, turning later src swaps into
  // memory-cache hits (no fetch, no re-decode).
  function updatePrefetchBar() {
    prefetchBar.value = prefetchDone;
    prefetchLabel.textContent = `${prefetchDone}/${prefetchTotal}`;
  }
  function onPrefetchSettled() {
    prefetchDone += 1;
    if (!prefetchShowBar) return;
    updatePrefetchBar();
    if (prefetchDone === prefetchTotal) {
      // Announce completion in the title, drop the now-redundant count, then
      // hold the full bar a moment before fading it out.
      sidebarTitle.textContent = 'Caching finished';
      prefetchLabel.textContent = '';
      setTimeout(() => prefetchEl.classList.add('done'), 800);
    }
  }
  // Pull URLs off the queue up to the concurrency cap. A no-op while paused, so
  // a freshly selected layer (see prioritizeActiveLoad) gets the link to itself.
  function pumpPrefetch() {
    if (prefetchPaused) return;
    while (prefetchInFlight < PREFETCH_CONCURRENCY && prefetchQueue.length) {
      const url = prefetchQueue.shift();
      prefetchInFlight += 1;
      const img = new Image();
      prefetched.set(url, img);
      img.onload = img.onerror = () => {
        prefetchInFlight -= 1;
        onPrefetchSettled();
        pumpPrefetch();
      };
      img.fetchPriority = 'low'; // let the browser favor the visible layer images
      img.src = url;
    }
  }
  // Whenever the shown layer changes, give it the connection to itself: pause
  // background warming until the images currently on screen finish loading.
  function prioritizeActiveLoad() {
    const gen = (activeLoadGen += 1);
    prefetchPaused = true;
    let pending = 0;
    const resume = () => {
      if (gen !== activeLoadGen || pending > 0) return; // superseded, or still loading
      prefetchPaused = false;
      pumpPrefetch();
    };
    for (const im of [layerBase, layerHead, layerEdge]) {
      if (!im.getAttribute('src') || im.complete) continue; // no source, or already loaded
      pending += 1;
      const onDone = () => {
        im.removeEventListener('load', onDone);
        im.removeEventListener('error', onDone);
        pending -= 1;
        resume();
      };
      im.addEventListener('load', onDone);
      im.addEventListener('error', onDone);
    }
    resume(); // nothing pending -> resume immediately
  }
  function startPrefetch() {
    const urls = new Set();
    for (const e of entries) {
      if (e.status !== 'added') urls.add(`a/svg/${e.path}`);
      if (e.status !== 'removed') urls.add(`b/svg/${e.path}`);
      if (e.edge) urls.add(e.edge);
    }
    prefetchTotal = urls.size;
    if (prefetchTotal === 0) return;
    // Only show the progress bar when there's enough to warm that the wait is
    // noticeable; small diffs cache instantly and don't need the chrome.
    prefetchShowBar = prefetchTotal >= PREFETCH_BAR_MIN;
    if (prefetchShowBar) {
      prefetchBar.max = prefetchTotal;
      // Width transitions on the bar also bubble transitionend up here, so only
      // collapse the element once its own opacity fade (the `done` class) ends.
      prefetchEl.addEventListener('transitionend', (ev) => {
        if (ev.target === prefetchEl && ev.propertyName === 'opacity') prefetchEl.hidden = true;
      });
      prefetchEl.hidden = false;
      updatePrefetchBar();
    }
    prefetchQueue = [...urls];
    pumpPrefetch(); // stays parked if the initial layer is still loading
  }
  // Defer past first paint so the initially selected page renders first.
  if (window.requestIdleCallback) requestIdleCallback(startPrefetch);
  else setTimeout(startPrefetch, 200);
})();
