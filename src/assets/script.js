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
  const layerEdgeBase = document.getElementById('layer-edge-base');
  const layerEdgeHead = document.getElementById('layer-edge-head');
  const divider = document.getElementById('swipe-divider');
  const marker = document.getElementById('marker');
  const placeholder = document.getElementById('placeholder');
  const modeButtons = document.querySelectorAll('.modes button');
  const mirrorToggle = document.getElementById('mirror-toggle');

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

  // Pan/zoom state. Each page fits to the viewport when opened; reset to
  // 1:1 with `0`.
  let scale = 1;
  let tx = 0;
  let ty = 0;
  // Set while the view should track the fitted state (see fitWhenReady);
  // any manual zoom/pan/reset takes over.
  let pendingFit = false;
  const MIN_SCALE = 0.1;
  const MAX_SCALE = 80;

  function applyTransform() {
    stage.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
    updateMarker();
  }

  // The change marker lives in stage-wrap coordinates so it keeps a constant
  // screen size; recompute its position from the stage transform. A change
  // carries a location per revision (fb/fh) since parts move (or exist only
  // on one side); show the one matching the revision on display, and no
  // marker at all when the part does not exist there (a ring over nothing
  // reads as a bug).
  let markerChange = null;
  let markerFrac = null;
  function markerFracFor(c) {
    // Swipe and R/G show both revisions at once: mark the part wherever it
    // exists, at its head location when it is on both sides.
    if (mode === 'rg' || mode === 'swipe') return c.fh || c.fb || null;
    const baseShown = mode === 'base' ||
      (mode === 'blink' && !stage.classList.contains('blink-head'));
    return (baseShown ? c.fb : c.fh) || null;
  }
  function refreshMarker() {
    if (!markerChange) return;
    markerFrac = markerFracFor(markerChange);
    marker.hidden = !markerFrac;
    updateMarker();
  }
  // Mirror view flips the layers around the stage midline; flip marker
  // x the same way so the ring stays on the part.
  function dispX(fx) {
    return stage.classList.contains('mirrored') ? 1 - fx : fx;
  }
  function updateMarker() {
    if (!markerFrac) return;
    marker.style.left = `${tx + dispX(markerFrac[0]) * stage.offsetWidth * scale}px`;
    marker.style.top = `${ty + markerFrac[1] * stage.offsetHeight * scale}px`;
  }
  function hideMarker() {
    markerChange = null;
    markerFrac = null;
    marker.hidden = true;
  }
  // Pan so the marker sits centered, keeping the current zoom (re-running
  // the focus zoom would stomp a manually chosen one).
  function centerMarker() {
    const wrapW = stageWrap.clientWidth;
    const wrapH = stageWrap.clientHeight;
    if (!markerFrac || !wrapW || !wrapH || !stage.offsetWidth) return;
    tx = wrapW / 2 - dispX(markerFrac[0]) * stage.offsetWidth * scale;
    ty = wrapH / 2 - markerFrac[1] * stage.offsetHeight * scale;
    applyTransform();
  }

  function resetView() {
    pendingFit = false;
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

  // Fit the just-selected page. The layers load asynchronously and the
  // moment the stage's layout reflects the new image differs per browser
  // (Firefox settles the very first page after the img load event), so no
  // single moment is safe to measure at. Fit immediately with whatever
  // size the stage has, again when the size-driving image loads, and keep
  // re-fitting while the stage or the viewport settles, until the user
  // takes over the view (zoom, pan or reset).
  function refit() {
    if (pendingFit) fitView();
  }
  function fitWhenReady() {
    pendingFit = true;
    fitView();
    const driver = layerBase.getAttribute('src') ? layerBase : layerHead;
    if (driver.getAttribute('src') && !driver.complete) {
      driver.addEventListener('load', refit, { once: true });
      driver.addEventListener('error', refit, { once: true });
    }
  }
  const refitter = new ResizeObserver(refit);
  refitter.observe(stage);
  refitter.observe(stageWrap);

  // Run `action` once the size-driving image of the current page is ready.
  // The layers load asynchronously, so acting right away would measure the
  // previous page; wait for the decode when the image is not cached yet.
  // The generation counter drops stale actions when the user has already
  // moved on.
  let readyGen = 0;
  function whenStageReady(action) {
    const gen = ++readyGen;
    const driver = layerBase.getAttribute('src') ? layerBase : layerHead;
    if (!driver.getAttribute('src') || driver.complete) {
      action();
      return;
    }
    const onDone = () => {
      driver.removeEventListener('load', onDone);
      driver.removeEventListener('error', onDone);
      if (gen === readyGen) action();
    };
    driver.addEventListener('load', onDone);
    driver.addEventListener('error', onDone);
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
      // The marked part may sit elsewhere (or on one side only); follow the
      // revision being flashed.
      refreshMarker();
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
    // The marked part may sit elsewhere (or not exist) on the newly shown
    // revision; follow it. When its location changed (or it just appeared),
    // also pan there so comparing sides never needs a re-click. A part that
    // stayed put keeps the view still, as does blink, whose marker
    // alternates instead.
    const before = markerFrac;
    refreshMarker();
    const movedTo = markerFrac &&
      (!before || markerFrac[0] !== before[0] || markerFrac[1] !== before[1]);
    if (movedTo) centerMarker();
    updateHash();
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

    // Board-outline context for PCB layers (already index-relative URLs).
    // One overlay per side: each is shown/hidden by the same mode rules as
    // its revision's layer, so flipping base/head also flips the outline.
    const setEdge = (img, url, cls) => {
      img.src = url || '';
      stage.classList.toggle(cls, !!url);
    };
    setEdge(layerEdgeBase, entry.edgeBase, 'has-edge-base');
    setEdge(layerEdgeHead, entry.edgeHead, 'has-edge-head');
    // Schematic and PCB pages get theme filters applied differently.
    stage.dataset.kind = entry.kind;

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
    const li = liElements[idx];
    li.classList.add('active');
    // Unfold any collapsed sections above the newly selected entry.
    for (let el = li.parentElement; el; el = el.parentElement) {
      if (el.tagName === 'DETAILS' && !el.open) el.open = true;
    }
    const entry = entries[idx];
    hideMarker();
    // A manually chosen page no longer matches the focused change; drop the
    // stale row highlight (goToChange re-applies it after selecting).
    changeLis.forEach((li) => li.classList.remove('active'));
    changeShown = false;
    applyAvailability(entry);
    fitWhenReady();
    updateHash();
  }

  // The URL hash mirrors the view - page, focused change, compare mode - so
  // the address bar is always a shareable link to what is on screen. Changes
  // are addressed by reference + detail, which survives report regeneration
  // (row numbers do not); a link whose target no longer exists just opens
  // the report normally.
  let changeShown = false;
  const pageKey = (e) => `${e.project}/${e.kind}/${e.name}`;
  function updateHash() {
    if (activeIdx === null) return;
    const params = new URLSearchParams();
    params.set('p', pageKey(entries[activeIdx]));
    const c = changeShown ? allChanges[activeChange] : null;
    if (c) params.set('c', `${c.ref}|${c.detail}`);
    params.set('m', mode);
    history.replaceState(null, '', `#${params.toString()}`);
  }
  function applyHash() {
    const params = new URLSearchParams(location.hash.slice(1));
    const pageIdx = entries.findIndex((e) => pageKey(e) === params.get('p'));
    const key = params.get('c');
    const changeIdx = key === null ? -1 : allChanges.findIndex(
      (c) => `${c.ref}|${c.detail}` === key
        && (pageIdx < 0 || c.project === entries[pageIdx].project),
    );
    if (changeIdx >= 0 && changeHome(allChanges[changeIdx]) >= 0) {
      goToChange(changeIdx);
    } else {
      select(pageIdx >= 0 ? pageIdx : 0);
    }
    const m = params.get('m');
    if (m) {
      const btn = [...modeButtons].find((b) => b.dataset.mode === m);
      if (btn && !btn.disabled) setMode(m);
    }
  }
  window.addEventListener('hashchange', applyHash);

  // Semantic (part-level) changes nest under the page they happened on, as a
  // collapsed group behind a summary line. Clicking a change row (or stepping
  // with n/p, which expands the group it enters) jumps to that page, zooms in
  // on the location, and shows the pulsing marker.
  const CHANGE_LETTERS = { added: 'A', removed: 'R', renamed: 'N', moved: 'M', flipped: 'S', value: 'V', footprint: 'F', property: 'P', net: 'C' };
  const CHANGE_BADGES = { added: 'added', removed: 'removed', net: 'net' }; // rest fall back to "modified" colors
  const allChanges = typeof changes !== 'undefined' ? changes : [];
  let activeChange = -1;
  const changeLis = new Map();    // change index -> its row
  const changeGroups = new Map(); // entry index -> its group's DOM + open state
  const changeOrder = [];         // change indices in sidebar order, walked by n/p

  // The page a change lives under in the sidebar, which is also where
  // clicking it navigates: the sheet for schematic changes, the part's copper
  // layer for board changes (or the board's first layer when that page is not
  // in the report).
  function changeHome(c) {
    if (c.scope === 'sch') {
      return entries.findIndex((e) => e.project === c.project && e.kind === 'sch' && e.name === c.sheet);
    }
    const layerPage = entries.findIndex(
      (e) => e.project === c.project && e.kind === 'pcb' && e.name === c.layer,
    );
    if (layerPage >= 0) return layerPage;
    return entries.findIndex((e) => e.project === c.project && e.kind === 'pcb');
  }

  function setGroupOpen(idx, open) {
    const g = changeGroups.get(idx);
    if (!g || g.open === open) return;
    g.open = open;
    g.row.classList.toggle('open', open);
    g.summary.hidden = open;
    g.items.hidden = !open;
  }

  function highlightChange() {
    changeLis.forEach((li, idx) => li.classList.toggle('active', idx === activeChange));
    const li = changeLis.get(activeChange);
    if (li) li.scrollIntoView({ block: 'nearest' });
  }

  // Zoom in on the change location, centered, and show the marker there.
  // A part that does not exist on the displayed revision can only be looked
  // at on the other one; switch to it. Blink keeps alternating instead and
  // the marker flashes along with the side the part exists on. Focusing
  // takes over the view like a manual zoom does.
  function focusChange(c) {
    pendingFit = false;
    let frac = markerFracFor(c);
    if (!frac && (c.fb || c.fh)) {
      if (mode === 'blink') {
        frac = c.fb || c.fh;
      } else {
        setMode(c.fb ? 'base' : 'head');
        frac = markerFracFor(c);
      }
    }
    if (!frac) {
      hideMarker();
      return;
    }
    const stageW = stage.offsetWidth;
    const stageH = stage.offsetHeight;
    const wrapW = stageWrap.clientWidth;
    const wrapH = stageWrap.clientHeight;
    if (!stageW || !stageH || !wrapW || !wrapH) return;
    const fitK = Math.min(wrapW / stageW, wrapH / stageH) * 0.96;
    scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, fitK * 4));
    tx = wrapW / 2 - dispX(frac[0]) * stageW * scale;
    ty = wrapH / 2 - frac[1] * stageH * scale;
    markerChange = c;
    refreshMarker();
    applyTransform();
  }

  function goToChange(i) {
    const c = allChanges[i];
    if (!c) return;
    const target = changeHome(c);
    if (target < 0) return;
    activeChange = i;
    setGroupOpen(target, true);
    if (target !== activeIdx) select(target);
    highlightChange();
    whenStageReady(() => focusChange(c));
    changeShown = true;
    updateHash();
  }

  // Clicking the focused change again releases it: the marker stops pulsing
  // and the row highlight drops. The view stays where it is.
  function clearChange() {
    activeChange = -1;
    changeShown = false;
    hideMarker();
    highlightChange();
    updateHash();
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
    // Clip-path lives on the mirrored layers, so invert its position when
    // mirrored so the visual split still lands under the cursor. The divider
    // sits on the un-mirrored stage and needs no adjustment.
    const clipX = stage.classList.contains('mirrored') ? stageW - clamped : clamped;
    const pct = (clipX / stageW) * 100;
    stage.style.setProperty('--swipe', `${pct}%`);
    divider.style.left = `${clamped}px`;
  });

  // Wheel zoom — anchored on cursor position. Scale exponentially by the
  // actual scroll delta (normalized across px/line/page wheel units and
  // clamped per event) so a trackpad two-finger flick, which fires many small
  // events, zooms at a similar rate to a mouse wheel instead of rocketing.
  stageWrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    pendingFit = false;
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
    if (e.button !== 0 && e.button !== 1) return;
    if (e.button === 1) e.preventDefault();
    dragging = true;
    pendingFit = false;
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

  function toggleMirror() {
    const on = stage.classList.toggle('mirrored');
    mirrorToggle.classList.toggle('active', on);
    mirrorToggle.setAttribute('aria-pressed', on ? 'true' : 'false');
    updateMarker();
  }
  mirrorToggle.addEventListener('click', toggleMirror);

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
    } else if (e.key === 'm') {
      toggleMirror();
    } else if (e.key === 'n' || e.key === 'p') {
      if (changeOrder.length) {
        const pos = changeOrder.indexOf(activeChange);
        const len = changeOrder.length;
        const at = pos < 0
          ? (e.key === 'n' ? 0 : len - 1)
          : (e.key === 'n' ? (pos + 1) % len : (pos - 1 + len) % len);
        goToChange(changeOrder[at]);
      }
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

  // Changes by the page they nest under. The sneaky ones lead each group:
  // properties (a different part gets mounted, no pixel moved), then net
  // rewires, then everything visible.
  const changesByPage = new Map();
  allChanges.forEach((c, i) => {
    const home = changeHome(c);
    if (home < 0) return;
    if (!changesByPage.has(home)) changesByPage.set(home, []);
    changesByPage.get(home).push(i);
  });
  const KIND_RANK = { property: 0, net: 1 };
  const rank = (i) => KIND_RANK[allChanges[i].kind] ?? 2;
  changesByPage.forEach((idxs) => idxs.sort((x, y) => rank(x) - rank(y) || x - y));

  // The group under one page row: a count chip and a chevron on the row
  // itself, then a summary line ("N changes · NN% of parts changed") that
  // expands into the individual change rows. The percentage counts unique
  // references with part changes against the parts on the page; net-only
  // groups get no percentage (nets have no such denominator).
  function appendChangeGroup(container, row, entryIdx, entry, idxs) {
    const chip = document.createElement('span');
    chip.className = 'count-chip';
    chip.textContent = idxs.length;
    const chevron = document.createElement('span');
    chevron.className = 'chevron';
    chevron.textContent = '▸';
    row.appendChild(chip);
    row.appendChild(chevron);

    const changedRefs = new Set();
    for (const ci of idxs) {
      const c = allChanges[ci];
      if (c.kind !== 'net') changedRefs.add(c.ref);
    }
    let text = idxs.length === 1 ? '1 change' : `${idxs.length} changes`;
    if (changedRefs.size && entry.parts) {
      // Board changes falling back to this page (their own layer is not in
      // the report) can push the count past this page's own part total.
      const pct = Math.min(100, Math.round((100 * changedRefs.size) / entry.parts));
      text += ` · ${pct}% of parts changed`;
    }
    const summary = document.createElement('div');
    summary.className = 'group-summary';
    summary.textContent = text;

    const items = document.createElement('ul');
    items.className = 'group-items';
    items.hidden = true;
    for (const ci of idxs) {
      const c = allChanges[ci];
      const li = document.createElement('li');
      const badge = document.createElement('span');
      badge.className = 'badge ' + (CHANGE_BADGES[c.kind] || 'modified');
      badge.textContent = CHANGE_LETTERS[c.kind] || '?';
      badge.title = c.kind;
      const label = document.createElement('span');
      label.className = 'path';
      // Details carry ASCII "->" (they double as share-link keys); render
      // the separator as an arrow.
      const text = c.detail ? `${c.ref} · ${c.detail}` : c.ref;
      label.textContent = text.replace(/ -> /g, ' → ');
      label.title = label.textContent;
      li.appendChild(badge);
      li.appendChild(label);
      li.onclick = () => {
        if (activeChange === ci && changeShown) clearChange();
        else goToChange(ci);
      };
      changeLis.set(ci, li);
      items.appendChild(li);
      changeOrder.push(ci);
    }

    const holder = document.createElement('li');
    holder.className = 'change-group';
    holder.appendChild(summary);
    holder.appendChild(items);
    container.appendChild(holder);

    changeGroups.set(entryIdx, { row, summary, items, open: false });
    const toggle = (ev) => {
      ev.stopPropagation(); // the row click underneath selects the page
      setGroupOpen(entryIdx, !changeGroups.get(entryIdx).open);
    };
    chip.onclick = toggle;
    chevron.onclick = toggle;
    summary.onclick = () => setGroupOpen(entryIdx, true);
  }

  const KIND_LABELS = { sch: 'Schematics', pcb: 'PCB layers', fp: 'Footprints', sym: 'Symbols' };

  // Group entries by project, then by kind, preserving original order.
  const groups = new Map();
  entries.forEach((e, i) => {
    if (!groups.has(e.project)) groups.set(e.project, new Map());
    const kinds = groups.get(e.project);
    if (!kinds.has(e.kind)) kinds.set(e.kind, []);
    kinds.get(e.kind).push(i);
  });

  function makeSection(cls, headerCls, label) {
    const li = document.createElement('li');
    const details = document.createElement('details');
    details.open = true;
    details.className = cls;
    const summary = document.createElement('summary');
    summary.className = headerCls;
    summary.textContent = label;
    const body = document.createElement('ul');
    details.appendChild(summary);
    details.appendChild(body);
    li.appendChild(details);
    return { li, body };
  }

  for (const [project, kinds] of groups) {
    const proj = makeSection('project', 'project-header', project);
    list.appendChild(proj.li);
    for (const [kind, indices] of kinds) {
      const kindSec = makeSection('kind', 'group-header', KIND_LABELS[kind] || kind);
      proj.body.appendChild(kindSec.li);
      for (const idx of indices) {
        const e = entries[idx];
        const li = document.createElement('li');
        li.className = 'entry';
        const badge = document.createElement('span');
        badge.className = 'badge ' + e.status;
        badge.textContent = BADGE_LETTERS[e.status] || '?';
        const name = document.createElement('span');
        name.className = 'path';
        name.textContent = e.name;
        li.appendChild(badge);
        li.appendChild(name);
        li.onclick = () => select(idx);
        kindSec.body.appendChild(li);
        liElements[idx] = li;
        if (changesByPage.has(idx)) appendChangeGroup(kindSec.body, li, idx, e, changesByPage.get(idx));
      }
    }
  }
  applyHash();

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
    for (const im of [layerBase, layerHead, layerEdgeBase, layerEdgeHead]) {
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
      if (e.edgeBase) urls.add(e.edgeBase);
      if (e.edgeHead) urls.add(e.edgeHead);
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
