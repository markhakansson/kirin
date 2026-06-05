(() => {
  const list = document.getElementById('file-list');
  const stage = document.getElementById('stage');
  const stageWrap = stage.parentElement;
  const layerBase = document.getElementById('layer-base');
  const layerHead = document.getElementById('layer-head');
  const divider = document.getElementById('swipe-divider');
  const placeholder = document.getElementById('placeholder');
  const modeButtons = document.querySelectorAll('.modes button');

  const BADGE_LETTERS = { added: 'A', removed: 'R', modified: 'M', unchanged: 'U' };
  const MODES = ['base', 'head', 'onion', 'swipe', 'diff'];

  let activeIdx = null;
  let mode = 'head';
  const liElements = [];

  // Pan/zoom state. Persists across file switches; reset with `0`.
  let scale = 1;
  let tx = 0;
  let ty = 0;
  const MIN_SCALE = 0.1;
  const MAX_SCALE = 30;

  function applyTransform() {
    stage.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
  }

  function resetView() {
    scale = 1;
    tx = 0;
    ty = 0;
    applyTransform();
  }

  function setMode(m) {
    if (!MODES.includes(m)) return;
    mode = m;
    stage.dataset.mode = m;
    stageWrap.classList.toggle('swipe-cursor', m === 'swipe');
    modeButtons.forEach((b) => b.classList.toggle('active', b.dataset.mode === m));
  }

  // Change currently active layer.
  function flipLayer() {
    setMode(mode === 'base' ? 'head' : 'base');
  }

  function applyAvailability(entry) {
    const hasBase = entry.status !== 'added';
    const hasHead = entry.status !== 'removed';

    layerBase.src = hasBase ? `a/svg/${entry.kind}/${entry.path}` : '';
    layerHead.src = hasHead ? `b/svg/${entry.kind}/${entry.path}` : '';

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
      const needsBoth = m === 'onion' || m === 'swipe' || m === 'diff';
      b.disabled = (needsBoth && (!hasBase || !hasHead)) ||
                   (m === 'base' && !hasBase) ||
                   (m === 'head' && !hasHead);
    });
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

  // Wheel zoom — anchored on cursor position.
  stageWrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    const rect = stageWrap.getBoundingClientRect();
    const cx = e.clientX - rect.left;
    const cy = e.clientY - rect.top;
    const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
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

  // Mode buttons
  modeButtons.forEach((b) => {
    b.addEventListener('click', () => {
      if (!b.disabled) setMode(b.dataset.mode);
    });
  });

  // Keyboard shortcuts
  document.addEventListener('keydown', (e) => {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
    const map = { '1': 'base', '2': 'head', '3': 'onion', '4': 'swipe', '5': 'diff' };
    if (map[e.key]) {
      const btn = [...modeButtons].find((b) => b.dataset.mode === map[e.key]);
      if (btn && !btn.disabled) setMode(map[e.key]);
    } else if (e.key === ' ') {
      e.preventDefault();
      flipLayer();
    } else if (e.key === '0') {
      resetView();
    } else if (e.key === 'ArrowDown' || e.key === 'j') {
      if (activeIdx !== null && activeIdx + 1 < entries.length) select(activeIdx + 1);
    } else if (e.key === 'ArrowUp' || e.key === 'k') {
      if (activeIdx !== null && activeIdx > 0) select(activeIdx - 1);
    }
  });

  if (entries.length === 0) {
    document.querySelector('main').innerHTML = '<div class="nothing">No schematics found.</div>';
    return;
  }

  const KIND_LABELS = { sch: 'Schematics', pcb: 'PCBs' };
  let currentKind = null;
  entries.forEach((e, i) => {
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
    const path = document.createElement('span');
    path.className = 'path';
    path.textContent = e.path;
    li.appendChild(badge);
    li.appendChild(path);
    li.onclick = () => select(i);
    list.appendChild(li);
    liElements[i] = li;
  });
  select(0);
})();
