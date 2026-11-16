'use strict';

/* ---------------------------------------------------------------------------
 * Splashhound frontend — plain JS against the Tauri command bridge.
 * ------------------------------------------------------------------------- */

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

const state = {
  boot: null,
  media: '',
  stats: {},
  tags: [],
  creators: [],
  scan: null,
  scanTimer: null,
  view: '',
  playerToken: 0,
  lib: { filter: null, items: [], total: 0, loading: false },
  rate: { queue: [], index: 0, loading: false, saving: false, editor: null, video: null },
  dups: [],
  keep: {},
};

const FLAGS = [
  ['nsfw', 'NSFW'],
  ['popping', 'Popping'],
  ['destruction', 'Destruction'],
  ['deflation', 'Deflation'],
  ['inflation', 'Inflation'],
];

/* ----------------------------- helpers ---------------------------------- */

function esc(value) {
  return String(value ?? '').replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function fmtSize(bytes) {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i += 1; }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function fmtDur(ms) {
  const total = Math.round((ms || 0) / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
           : `${m}:${String(s).padStart(2, '0')}`;
}

function fmtClock(seconds) {
  const total = Math.max(0, Math.floor(seconds || 0));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function fmtDate(unix) {
  if (!unix) return '—';
  return new Date(unix * 1000).toLocaleDateString();
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function toast(message, kind = 'info') {
  const el = document.createElement('div');
  el.className = `toast ${kind === 'error' ? 'error' : ''}`;
  el.textContent = message;
  $('#toasts').appendChild(el);
  setTimeout(() => {
    el.classList.add('out');
    setTimeout(() => el.remove(), 300);
  }, kind === 'error' ? 6000 : 3200);
}

function phaseLabel(phase) {
  return ({
    starting: 'Starting',
    walking: 'Walking folders',
    indexing: 'Indexing files',
    probing: 'Reading metadata',
    hashing: 'Hashing files',
    done: 'Done',
    error: 'Error',
  }[phase] || phase || '');
}

function refreshChrome() {
  const s = state.stats || {};
  $('#count-library').textContent = s.total ?? 0;
  $('#count-rate').textContent = s.unreviewed ?? 0;
  $('#count-dups').textContent = s.duplicates ?? 0;
  const pct = s.total ? Math.round((100 * (s.reviewed || 0)) / s.total) : 0;
  $('#side-progress').style.width = `${pct}%`;
  $('#side-progress-text').textContent = `${s.reviewed || 0} / ${s.total || 0} reviewed`;
  updateScanUI();
}

async function refreshMeta() {
  try {
    const [stats, tags, creators] = await Promise.all([
      invoke('stats'), invoke('all_tags'), invoke('creators'),
    ]);
    state.stats = stats;
    state.tags = tags;
    state.creators = creators;
    refreshChrome();
  } catch (err) {
    console.warn('refreshMeta failed', err);
  }
}

/* ------------------------------ modal ----------------------------------- */

let modalCloser = null;

function modal(html, options = {}) {
  closeAutocomplete();
  const root = $('#modal-root');
  root.innerHTML = `<div class="modal-backdrop"><div class="modal ${options.className || ''}">${html}</div></div>`;
  const close = () => {
    if (!root.innerHTML) return;
    root.innerHTML = '';
    modalCloser = null;
    if (options.onClose) options.onClose();
  };
  modalCloser = close;
  const backdrop = $('.modal-backdrop', root);
  backdrop.addEventListener('mousedown', (e) => { if (e.target === backdrop) close(); });
  $$('[data-close]', root).forEach((b) => b.addEventListener('click', close));
  if (options.onMount) options.onMount(root, close);
  return close;
}

function confirmDialog({ title, message, okLabel = 'Confirm', danger = false }) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (!settled) { settled = true; resolve(value); }
    };
    modal(
      `<h2>${esc(title)}</h2>
       <div class="modal-body"><p>${esc(message)}</p></div>
       <div class="modal-actions">
         <button class="btn" data-close>Cancel</button>
         <button class="btn ${danger ? 'danger' : 'primary'}" id="dlg-ok">${esc(okLabel)}</button>
       </div>`,
      {
        onMount(root, close) {
          $('#dlg-ok', root).onclick = () => { finish(true); close(); };
          $$('[data-close]', root).forEach((b) => b.addEventListener('click', () => finish(false)));
          $('.modal-backdrop', root).addEventListener('mousedown', (e) => {
            if (e.target.classList.contains('modal-backdrop')) finish(false);
          });
        },
        onClose() { finish(false); },
      },
    );
  });
}

/* ------------------------------- init ----------------------------------- */

window.addEventListener('DOMContentLoaded', init);

async function init() {
  $$('.nav-item').forEach((b) => b.addEventListener('click', () => navigate(b.dataset.view)));
  try {
    const boot = await invoke('bootstrap');
    state.boot = boot;
    state.media = boot.mediaBase;
    state.stats = boot.stats;
    state.tags = boot.tags;
    state.creators = boot.creators;
    state.scan = boot.scan;
    refreshChrome();
    if (state.scan?.running) startScanPolling();
    navigate('library');
  } catch (err) {
    $('#view').innerHTML = `<div class="fatal">Splashhound failed to start.\n\n${esc(String(err))}</div>`;
  }
}

function navigate(view) {
  closeAutocomplete();
  if (state.view === view) {
    if (view === 'library') loadLibrary(true);
    return;
  }
  if (state.view === 'rate') stopRatePlayer();
  state.view = view;
  $$('.nav-item').forEach((b) => b.classList.toggle('active', b.dataset.view === view));
  $('#main').scrollTop = 0;
  if (view === 'library') viewLibrary();
  else if (view === 'rate') viewRate();
  else if (view === 'duplicates') viewDuplicates();
  else if (view === 'settings') viewSettings();
}

function rerenderView() {
  if (state.view === 'library') loadLibrary(true);
  else if (state.view === 'rate') loadRateQueue();
  else if (state.view === 'duplicates') viewDuplicates();
  else if (state.view === 'settings') viewSettings();
}

/* ------------------------------ library --------------------------------- */

function defaultFilter() {
  return {
    search: '',
    tags: [],
    tagMode: 'any',
    nsfw: 'any',
    reviewed: 'any',
    minRating: 0,
    creator: '',
    sort: 'added_desc',
    limit: 60,
    offset: 0,
  };
}

function viewLibrary() {
  if (!state.lib.filter) state.lib.filter = defaultFilter();
  const f = state.lib.filter;
  const creatorOptions = ['<option value="">Any creator</option>']
    .concat(state.creators.map((c) => `<option value="${esc(c)}" ${c === f.creator ? 'selected' : ''}>${esc(c)}</option>`))
    .join('');

  $('#view').innerHTML = `
    <header class="page-head">
      <div>
        <h1>Library</h1>
        <p class="muted" id="lib-sub">Loading…</p>
      </div>
      <div class="head-actions">
        <button class="btn" id="lib-reshuffle">Shuffle</button>
        <button class="btn" id="lib-refresh">Refresh</button>
        <button class="btn primary" id="lib-scan">Scan folder</button>
      </div>
    </header>

    <div class="toolbar">
      <input class="input search" id="f-search" placeholder="Search file name, path, creator, notes…" value="${esc(f.search)}">
      <select class="select" id="f-nsfw">
        <option value="any">NSFW: any</option>
        <option value="yes">NSFW only</option>
        <option value="no">SFW only</option>
      </select>
      <select class="select" id="f-reviewed">
        <option value="any">Reviewed: any</option>
        <option value="yes">Reviewed</option>
        <option value="no">Unreviewed</option>
      </select>
      <select class="select" id="f-rating">
        <option value="0">Rating: any</option>
        <option value="1">★ 1+</option>
        <option value="2">★ 2+</option>
        <option value="3">★ 3+</option>
        <option value="4">★ 4+</option>
        <option value="5">★ 5</option>
      </select>
      <select class="select" id="f-creator">${creatorOptions}</select>
      <select class="select" id="f-sort">
        <option value="added_desc">Newest added</option>
        <option value="name_asc">Name A → Z</option>
        <option value="rating_desc">Rating high → low</option>
        <option value="views_desc">Most viewed</option>
        <option value="duration_desc">Longest</option>
        <option value="size_desc">Largest file</option>
        <option value="random">Random</option>
      </select>
      <button class="btn" id="f-tags-btn">Tags <span class="pill" id="tag-count">0</span></button>
    </div>

    <div class="tag-filter hidden" id="tag-filter"></div>
    <div class="chips" id="active-filters" style="margin-bottom:12px"></div>

    <div class="grid" id="lib-grid"></div>
    <div class="load-more"><button class="btn" id="lib-more" style="display:none">Load more</button></div>
  `;

  $('#f-nsfw').value = f.nsfw;
  $('#f-reviewed').value = f.reviewed;
  $('#f-rating').value = String(f.minRating);
  $('#f-sort').value = f.sort;

  let searchTimer = null;
  $('#f-search').addEventListener('input', (e) => {
    f.search = e.target.value;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => loadLibrary(true), 280);
  });
  $('#f-nsfw').addEventListener('change', (e) => { f.nsfw = e.target.value; loadLibrary(true); });
  $('#f-reviewed').addEventListener('change', (e) => { f.reviewed = e.target.value; loadLibrary(true); });
  $('#f-rating').addEventListener('change', (e) => { f.minRating = Number(e.target.value); loadLibrary(true); });
  $('#f-creator').addEventListener('change', (e) => { f.creator = e.target.value; loadLibrary(true); });
  $('#f-sort').addEventListener('change', (e) => { f.sort = e.target.value; loadLibrary(true); });

  $('#f-tags-btn').addEventListener('click', () => {
    $('#tag-filter').classList.toggle('hidden');
  });
  $('#lib-refresh').addEventListener('click', () => { loadLibrary(true); refreshMeta(); });
  $('#lib-reshuffle').addEventListener('click', () => {
    f.sort = 'random';
    $('#f-sort').value = 'random';
    loadLibrary(true);
  });
  $('#lib-scan').addEventListener('click', startScan);
  $('#lib-more').addEventListener('click', () => loadLibrary(false));

  const grid = $('#lib-grid');
  grid.addEventListener('click', (e) => {
    const card = e.target.closest('.card');
    if (card) openDetail(Number(card.dataset.id));
  });

  $('#tag-count').textContent = f.tags.length;
  renderTagFilter();
  renderActiveFilters();
  loadLibrary(true);
}

function renderTagFilter() {
  const panel = $('#tag-filter');
  if (!panel) return;
  const f = state.lib.filter;
  panel.innerHTML = `
    <div class="tag-filter-head">
      <strong>Filter by tag</strong>
      <div class="row gap">
        <button class="btn tiny" id="tag-mode">${f.tagMode === 'all' ? 'Match all' : 'Match any'}</button>
        <button class="btn tiny" id="tag-clear">Clear</button>
      </div>
    </div>
    <div class="chips">
      ${state.tags.length
        ? state.tags.map((t) => `<button class="chip ${f.tags.includes(t.name) ? 'active' : ''}" data-tag="${esc(t.name)}">${esc(t.name)} <span class="n">${t.count}</span></button>`).join('')
        : '<span class="muted small">No tags yet — add them while rating videos.</span>'}
    </div>`;
  $$('[data-tag]', panel).forEach((btn) => {
    btn.onclick = () => {
      const name = btn.dataset.tag;
      const idx = f.tags.indexOf(name);
      if (idx >= 0) f.tags.splice(idx, 1);
      else f.tags.push(name);
      $('#tag-count').textContent = f.tags.length;
      renderTagFilter();
      renderActiveFilters();
      loadLibrary(true);
    };
  });
  $('#tag-mode', panel).onclick = () => {
    f.tagMode = f.tagMode === 'all' ? 'any' : 'all';
    renderTagFilter();
    loadLibrary(true);
  };
  $('#tag-clear', panel).onclick = () => {
    f.tags = [];
    $('#tag-count').textContent = '0';
    renderTagFilter();
    renderActiveFilters();
    loadLibrary(true);
  };
}

function renderActiveFilters() {
  const wrap = $('#active-filters');
  if (!wrap) return;
  const f = state.lib.filter;
  wrap.innerHTML = f.tags.map((t) => `
      <span class="chip active">${esc(t)}<button class="chip-x" data-remove="${esc(t)}">×</button></span>`).join('');
  $$('[data-remove]', wrap).forEach((b) => {
    b.onclick = () => {
      f.tags = f.tags.filter((t) => t !== b.dataset.remove);
      $('#tag-count').textContent = f.tags.length;
      renderTagFilter();
      renderActiveFilters();
      loadLibrary(true);
    };
  });
}

async function loadLibrary(reset) {
  const grid = $('#lib-grid');
  if (!grid) return;
  if (state.lib.loading) return;
  const f = state.lib.filter;
  if (reset) {
    state.lib.items = [];
    f.offset = 0;
    grid.innerHTML = '';
  }
  state.lib.loading = true;
  grid.classList.add('loading');
  try {
    const res = await invoke('list_videos', {
      filter: { ...f, limit: 60, offset: f.offset },
    });
    state.lib.total = res.total;
    state.lib.items = reset ? res.items : state.lib.items.concat(res.items);
    f.offset = state.lib.items.length;
    renderLibraryGrid();
  } catch (err) {
    toast(`Could not load library: ${err}`, 'error');
  } finally {
    state.lib.loading = false;
    grid.classList.remove('loading');
  }
}

function renderLibraryGrid() {
  const grid = $('#lib-grid');
  if (!grid) return;
  const items = state.lib.items;
  const total = state.lib.total;
  const sub = $('#lib-sub');
  if (sub) {
    const filtered = state.lib.filter.tags.length || state.lib.filter.search || state.lib.filter.nsfw !== 'any'
      || state.lib.filter.reviewed !== 'any' || state.lib.filter.minRating || state.lib.filter.creator;
    sub.textContent = filtered
      ? `${items.length} of ${total} matching videos`
      : `${total} videos · ${fmtSize(state.stats.totalSize || 0)}`;
  }
  const more = $('#lib-more');
  if (more) more.style.display = items.length < total && state.lib.filter.sort !== 'random' ? '' : 'none';

  if (!items.length) {
    grid.innerHTML = `<div class="empty" style="grid-column: 1/-1">
        <h2>Nothing here yet</h2>
        <p>${state.boot?.root ? 'No videos match these filters.' : 'Pick your library folder in Settings, then run a scan.'}</p>
        <p><button class="btn primary" id="empty-scan">${state.boot?.root ? 'Open Settings' : 'Choose folder'}</button></p>
      </div>`;
    $('#empty-scan').onclick = () => navigate('settings');
    return;
  }

  grid.innerHTML = items.map(cardHtml).join('');
  $$('.poster img', grid).forEach((img) => {
    img.addEventListener('error', () => img.classList.add('broken'));
  });
}

function cardHtml(v) {
  const badges = FLAGS.filter(([k]) => v[k])
    .map(([k, label]) => `<span class="badge ${k === 'nsfw' ? 'nsfw' : ''}">${label}</span>`)
    .join('');
  const res = v.width ? `${v.width}×${v.height}` : '';
  return `
    <article class="card" data-id="${v.id}">
      <div class="poster">
        <img loading="lazy" src="${state.media}/poster/${v.id}?k=${v.mtime}" alt="">
        <span class="dur">${v.durationMs ? fmtDur(v.durationMs) : '—'}</span>
        ${v.rating ? `<span class="rating-badge">★ ${v.rating}</span>` : ''}
        ${v.reviewed ? '' : '<span class="new-badge">new</span>'}
      </div>
      <div class="card-body">
        <div class="card-title" title="${esc(v.fileName)}">${esc(v.fileName)}</div>
        <div class="card-meta">${[res, fmtSize(v.size), v.creator].filter(Boolean).join(' · ')}</div>
        ${badges ? `<div class="badge-row">${badges}</div>` : ''}
      </div>
    </article>`;
}

/* -------------------------- autocomplete popup ---------------------------- */

let autocompleteCloser = null;

function closeAutocomplete() {
  const closer = autocompleteCloser;
  autocompleteCloser = null;
  if (closer) closer();
}

/**
 * Themed suggestion list for inputs (replaces the OS-styled <datalist>).
 * getItems(query) returns [{ value, meta, create }].
 */
function attachAutocomplete(input, getItems, options = {}) {
  let popup = null;
  let items = [];
  let active = -1;

  const close = () => {
    if (popup) {
      popup.remove();
      popup = null;
      window.removeEventListener('scroll', reposition, true);
      window.removeEventListener('resize', reposition);
    }
    items = [];
    active = -1;
    if (autocompleteCloser === close) autocompleteCloser = null;
  };

  function reposition() {
    if (!popup) return;
    const rect = input.getBoundingClientRect();
    popup.style.left = `${rect.left}px`;
    popup.style.top = `${rect.bottom + 4}px`;
    popup.style.width = `${rect.width}px`;
  }

  const render = () => {
    if (!popup) return;
    popup.innerHTML = items.map((item, i) => `
      <button type="button" class="ac-item ${i === active ? 'active' : ''}" data-i="${i}">
        <span class="ac-label">${item.create ? '+ ' : ''}${esc(item.value)}</span>
        ${item.meta !== undefined && item.meta !== null ? `<span class="ac-meta">${esc(item.meta)}</span>` : ''}
      </button>`).join('');
    $$('.ac-item', popup).forEach((el) => {
      el.addEventListener('mousedown', (e) => e.preventDefault());
      el.addEventListener('click', () => pick(Number(el.dataset.i)));
    });
    const current = $('.ac-item.active', popup);
    if (current) current.scrollIntoView({ block: 'nearest' });
  };

  const open = () => {
    items = (getItems(input.value.trim()) || []).slice(0, 9);
    if (!items.length) {
      close();
      return;
    }
    if (autocompleteCloser && autocompleteCloser !== close) closeAutocomplete();
    active = 0;
    if (!popup) {
      popup = document.createElement('div');
      popup.className = 'ac-popup';
      popup.addEventListener('mousedown', (e) => e.preventDefault());
      document.body.appendChild(popup);
      window.addEventListener('scroll', reposition, true);
      window.addEventListener('resize', reposition);
      autocompleteCloser = close;
    }
    reposition();
    render();
  };

  const pick = (index) => {
    const item = items[index];
    if (!item) return;
    close();
    if (options.onPick) options.onPick(item.value, item);
    input.focus();
  };

  input.addEventListener('input', open);
  input.addEventListener('focus', open);
  input.addEventListener('blur', () => setTimeout(close, 140));
  input.addEventListener('keydown', (e) => {
    if (!popup) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      active = (active + 1) % items.length;
      render();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      active = (active - 1 + items.length) % items.length;
      render();
    } else if (e.key === 'Enter') {
      if (active >= 0) {
        e.preventDefault();
        e.stopImmediatePropagation();
        pick(active);
      }
    } else if (e.key === 'Escape') {
      e.stopPropagation();
      close();
    } else if (e.key === 'Tab') {
      close();
    }
  });

  return { close };
}

/* ---------------------------- editor widget ------------------------------ */

function editorHtml(v, prefix, reviewedInitial) {
  const reviewed = reviewedInitial === undefined ? v.reviewed : reviewedInitial;
  return `
    <div class="field">
      <label>Creator</label>
      <input class="input" id="${prefix}-creator" placeholder="Unknown" value="${esc(v.creator || '')}">
    </div>
    <div class="field">
      <label>Flags</label>
      <div class="flags">
        ${FLAGS.map(([k, label]) => `<button type="button" class="flag" data-flag="${k}">${label}</button>`).join('')}
      </div>
    </div>
    <div class="field">
      <label>Rating</label>
      <div class="stars-row">
        <div class="stars">
          ${[1, 2, 3, 4, 5].map((n) => `<button type="button" data-star="${n}" title="${n} star${n > 1 ? 's' : ''}">★</button>`).join('')}
        </div>
        <button type="button" class="btn tiny" data-clear-rating>clear</button>
        <span class="muted small" id="${prefix}-rating-text"></span>
      </div>
    </div>
    <div class="field">
      <label>Tags</label>
      <div class="chips chips-static" id="${prefix}-tagchips"></div>
      <input class="input" id="${prefix}-taginput" placeholder="Type a tag and press Enter" style="margin-top:6px">
    </div>
    <div class="field">
      <label>Notes</label>
      <textarea class="input" id="${prefix}-notes" rows="3" placeholder="Optional notes…">${esc(v.notes || '')}</textarea>
    </div>
    <label class="check" style="margin-bottom:12px">
      <input type="checkbox" id="${prefix}-reviewed" ${reviewed ? 'checked' : ''}> Reviewed
    </label>`;
}

function createEditor(root, prefix, video) {
  const form = {
    flags: {
      nsfw: !!video.nsfw,
      popping: !!video.popping,
      destruction: !!video.destruction,
      deflation: !!video.deflation,
      inflation: !!video.inflation,
    },
    rating: video.rating || 0,
    tags: (video.tags || []).slice(),
  };

  const renderStars = () => {
    $$('[data-star]', root).forEach((b) => {
      b.classList.toggle('on', Number(b.dataset.star) <= form.rating);
    });
    const label = $(`#${prefix}-rating-text`, root);
    if (label) label.textContent = form.rating ? `${form.rating}/5` : '';
  };

  const renderTags = () => {
    const wrap = $(`#${prefix}-tagchips`, root);
    if (!wrap) return;
    wrap.innerHTML = form.tags.length
      ? form.tags.map((t, i) => `<span class="chip">${esc(t)}<button class="chip-x" data-tag-i="${i}" title="Remove">×</button></span>`).join('')
      : '<span class="muted small">No tags</span>';
  };

  $$('[data-flag]', root).forEach((btn) => {
    btn.classList.toggle('on', !!form.flags[btn.dataset.flag]);
    btn.onclick = () => {
      const key = btn.dataset.flag;
      form.flags[key] = !form.flags[key];
      btn.classList.toggle('on', form.flags[key]);
    };
  });

  $$('[data-star]', root).forEach((btn) => {
    btn.onclick = () => {
      const n = Number(btn.dataset.star);
      form.rating = form.rating === n ? 0 : n;
      renderStars();
    };
  });
  const clearBtn = $('[data-clear-rating]', root);
  if (clearBtn) clearBtn.onclick = () => { form.rating = 0; renderStars(); };

  const tagInput = $(`#${prefix}-taginput`, root);

  const addTag = (raw) => {
    const name = String(raw || '').trim().replace(/,+$/, '');
    if (!name) return;
    if (!form.tags.some((t) => t.toLowerCase() === name.toLowerCase())) {
      form.tags.push(name);
      renderTags();
    }
    tagInput.value = '';
  };

  attachAutocomplete(tagInput, (query) => {
    const q = query.toLowerCase();
    const taken = new Set(form.tags.map((t) => t.toLowerCase()));
    const matches = state.tags
      .filter((t) => !taken.has(t.name.toLowerCase()) && (!q || t.name.toLowerCase().includes(q)))
      .sort((a, b) => {
        const as = a.name.toLowerCase().startsWith(q) ? 0 : 1;
        const bs = b.name.toLowerCase().startsWith(q) ? 0 : 1;
        if (as !== bs) return as - bs;
        if (b.count !== a.count) return b.count - a.count;
        return a.name.localeCompare(b.name);
      })
      .map((t) => ({ value: t.name, meta: t.count }));
    if (q && !state.tags.some((t) => t.name.toLowerCase() === q) && !taken.has(q)) {
      matches.unshift({ value: query, meta: 'new', create: true });
    }
    return matches;
  }, { onPick: addTag });

  tagInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      e.stopPropagation();
      addTag(tagInput.value);
    } else if (e.key === 'Backspace' && !tagInput.value && form.tags.length) {
      form.tags.pop();
      renderTags();
    }
  });

  const creatorInput = $(`#${prefix}-creator`, root);
  attachAutocomplete(creatorInput, (query) => {
    const q = query.toLowerCase();
    return state.creators
      .filter((c) => !q || c.toLowerCase().includes(q))
      .sort((a, b) => {
        const as = a.toLowerCase().startsWith(q) ? 0 : 1;
        const bs = b.toLowerCase().startsWith(q) ? 0 : 1;
        return as - bs || a.localeCompare(b);
      })
      .map((c) => ({ value: c }));
  }, { onPick: (value) => { creatorInput.value = value; } });

  root.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-tag-i]');
    if (btn) {
      form.tags.splice(Number(btn.dataset.tagI), 1);
      renderTags();
    }
  });

  renderStars();
  renderTags();

  return {
    form,
    read() {
      const reviewedEl = $(`#${prefix}-reviewed`, root);
      return {
        creator: $(`#${prefix}-creator`, root).value,
        notes: $(`#${prefix}-notes`, root).value,
        nsfw: form.flags.nsfw,
        popping: form.flags.popping,
        destruction: form.flags.destruction,
        deflation: form.flags.deflation,
        inflation: form.flags.inflation,
        rating: form.rating,
        tags: form.tags.slice(),
        reviewed: reviewedEl ? reviewedEl.checked : undefined,
      };
    },
    toggleFlag(key) {
      if (!(key in form.flags)) return;
      form.flags[key] = !form.flags[key];
      const btn = $(`[data-flag="${key}"]`, root);
      if (btn) btn.classList.toggle('on', form.flags[key]);
    },
    setRating(n) {
      form.rating = form.rating === n ? 0 : n;
      renderStars();
    },
    focusTagInput() { tagInput.focus(); },
  };
}

/* ------------------------------ player ---------------------------------- */

function playVideo(videoEl, overlay, video, onReady) {
  const token = ++state.playerToken;
  state.rate.video = video;
  videoEl.pause();
  videoEl.removeAttribute('src');
  videoEl.load();
  overlay.classList.remove('hidden');
  overlay.innerHTML = '<div class="spinner"></div><div>Preparing…</div>';

  (async () => {
    try {
      let info = await invoke('prepare_playback', { id: video.id });
      while (token === state.playerToken && (info.state === 'working' || info.state === 'queued')) {
        const pct = info.progress >= 0 ? Math.round(info.progress * 100) : null;
        overlay.innerHTML = `
          <div class="spinner"></div>
          <div>Converting for playback… ${pct === null ? '' : pct + '%'}</div>
          <div class="progress slim"><div style="width:${pct === null ? 12 : pct}%"></div></div>
          <div class="muted small">This happens once per file and is cached.</div>`;
        await sleep(800);
        if (token !== state.playerToken) return;
        info = await invoke('playback_status', { id: video.id });
      }
      if (token !== state.playerToken) return;
      if (info.state !== 'ready' || !info.url) {
        overlay.classList.remove('hidden');
        overlay.innerHTML = `<div class="player-error">${esc(info.error || 'Playback failed.')}</div>`;
        return;
      }
      videoEl.src = `${info.url}?v=${Date.now()}`;
      overlay.classList.add('hidden');
      invoke('mark_viewed', { id: video.id }).catch(() => {});
      if (onReady) onReady();
      videoEl.play().catch(() => {});
    } catch (err) {
      if (token === state.playerToken) {
        overlay.classList.remove('hidden');
        overlay.innerHTML = `<div class="player-error">${esc(String(err))}</div>`;
      }
    }
  })();
}

async function loadThumbs(video, container, videoEl) {
  if (!container) return;
  container.innerHTML = Array.from({ length: 12 }, () => '<div class="thumb skeleton"></div>').join('');
  try {
    const res = await invoke('ensure_thumbs', { id: video.id });
    if (!res || !res.ok) {
      container.innerHTML = '<div class="muted small pad">Previews unavailable.</div>';
      return;
    }
    container.innerHTML = res.timestamps.map((t, i) => `
      <button class="thumb" data-t="${t}" title="Jump to ${fmtClock(t)}">
        <img loading="lazy" src="${state.media}/thumb/${video.id}/${i}?k=${video.mtime}" alt="">
        <span class="t-label">${fmtClock(t)}</span>
      </button>`).join('');
    $$('.thumb', container).forEach((btn) => {
      btn.onclick = () => {
        if (!videoEl) return;
        videoEl.currentTime = Number(btn.dataset.t);
        videoEl.play().catch(() => {});
      };
    });
    $$('.thumb img', container).forEach((img) => {
      img.addEventListener('error', () => { img.style.visibility = 'hidden'; });
    });
  } catch (err) {
    container.innerHTML = '<div class="muted small pad">Previews unavailable.</div>';
  }
}

/* ------------------------------ detail ---------------------------------- */

async function openDetail(id) {
  let v;
  try {
    v = await invoke('get_video', { id });
  } catch (err) {
    toast(String(err), 'error');
    return;
  }
  if (!v) return;

  modal(`
    <div class="detail">
      <div class="detail-left">
        <div class="player-wrap detail-player">
          <video id="d-video" controls playsinline preload="metadata"></video>
          <div class="player-overlay" id="d-overlay"><div class="spinner"></div></div>
        </div>
        <div class="thumb-strip" id="d-thumbs"></div>
      </div>
      <div class="detail-right">
        <h2 class="detail-title">${esc(v.fileName)}</h2>
        <div class="muted small">${esc(v.path)}</div>
        <dl class="meta-table">
          <dt>Duration</dt><dd>${v.durationMs ? fmtDur(v.durationMs) : '—'}</dd>
          <dt>Size</dt><dd>${fmtSize(v.size)}</dd>
          <dt>Resolution</dt><dd>${v.width ? `${v.width}×${v.height}` : '—'}</dd>
          <dt>Codecs</dt><dd>${esc(v.vcodec || '?')} / ${esc(v.acodec || 'no audio')}</dd>
          <dt>Playback</dt><dd>${esc(v.playback)}</dd>
          <dt>MD5</dt><dd class="mono">${esc(v.md5 || 'not hashed yet')}</dd>
          <dt>Views</dt><dd>${v.viewCount}</dd>
          <dt>Added</dt><dd>${fmtDate(v.addedAt)}</dd>
        </dl>
        ${editorHtml(v, 'd')}
        <div class="modal-actions">
          <button class="btn" id="d-reveal">Show in folder</button>
          <button class="btn" data-close>Close</button>
          <button class="btn primary" id="d-save">Save</button>
        </div>
      </div>
    </div>`, {
    className: 'wide',
    onClose() { state.playerToken += 1; },
    onMount(root) {
      const editor = createEditor(root, 'd', v);
      const videoEl = $('#d-video', root);
      playVideo(videoEl, $('#d-overlay', root), v);
      loadThumbs(v, $('#d-thumbs', root), videoEl);
      $('#d-reveal', root).onclick = () => {
        invoke('reveal', { id: v.id }).catch((err) => toast(String(err), 'error'));
      };
      $('#d-save', root).onclick = async () => {
        try {
          await invoke('save_video', { id: v.id, patch: editor.read() });
          toast('Saved.');
          refreshMeta();
        } catch (err) {
          toast(`Save failed: ${err}`, 'error');
        }
      };
    },
  });
}

/* ------------------------------- rate ----------------------------------- */

async function viewRate() {
  stopRatePlayer();
  $('#view').innerHTML = `
    <header class="page-head">
      <div>
        <h1>Rate</h1>
        <p class="muted" id="rate-sub">Loading queue…</p>
      </div>
      <div class="head-actions">
        <button class="btn" id="rate-reload">Reload queue</button>
      </div>
    </header>
    <div id="rate-body"><div class="empty">Loading…</div></div>`;
  $('#rate-reload').onclick = () => loadRateQueue();
  await loadRateQueue();
}

function stopRatePlayer() {
  const videoEl = $('#r-video');
  if (videoEl) {
    videoEl.pause();
    videoEl.removeAttribute('src');
    videoEl.load();
  }
  state.playerToken += 1;
  state.rate.editor = null;
}

async function loadRateQueue() {
  const body = $('#rate-body');
  if (!body) return;
  stopRatePlayer();
  body.innerHTML = '<div class="empty">Loading queue…</div>';
  try {
    const items = await invoke('rating_queue', { limit: 300 });
    state.rate.queue = items;
    state.rate.index = 0;
    if (!items.length) {
      body.innerHTML = `
        <div class="empty">
          <h2>All caught up</h2>
          <p>No unreviewed videos are left. New files show up here after a scan.</p>
          <p><button class="btn" id="rate-go-lib">Browse library</button></p>
        </div>`;
      $('#rate-go-lib').onclick = () => navigate('library');
      return;
    }
    renderRateCurrent();
  } catch (err) {
    body.innerHTML = `<div class="empty">Could not load the queue: ${esc(String(err))}</div>`;
  }
}

function renderRateCurrent() {
  closeAutocomplete();
  const r = state.rate;
  const body = $('#rate-body');
  if (!body) return;
  if (r.index >= r.queue.length) {
    body.innerHTML = `
      <div class="empty">
        <h2>Batch finished</h2>
        <p>You reviewed ${r.queue.length} videos in this batch.</p>
        <p><button class="btn primary" id="rate-more">Load more unreviewed</button></p>
      </div>`;
    $('#rate-more').onclick = () => { loadRateQueue(); refreshMeta(); };
    refreshMeta();
    return;
  }

  const v = r.queue[r.index];
  body.innerHTML = `
    <div class="rate-layout">
      <section class="rate-main">
        <div class="player-wrap">
          <video id="r-video" controls playsinline preload="metadata"></video>
          <div class="player-overlay" id="r-overlay"><div class="spinner"></div></div>
        </div>
        <div class="thumb-strip" id="r-thumbs"></div>
      </section>
      <aside class="rate-panel">
        <div class="small muted">${r.index + 1} of ${r.queue.length} in this batch</div>
        <h2 class="rate-title">${esc(v.fileName)}</h2>
        <dl class="meta-table">
          <dt>Duration</dt><dd>${v.durationMs ? fmtDur(v.durationMs) : '—'}</dd>
          <dt>Size</dt><dd>${fmtSize(v.size)}</dd>
          <dt>Resolution</dt><dd>${v.width ? `${v.width}×${v.height}` : '—'}</dd>
          <dt>Playback</dt><dd>${esc(v.playback)}</dd>
        </dl>
        <div id="r-editor">
          ${editorHtml(v, 'r', true)}
        </div>
        <div class="rate-actions">
          <button class="btn primary" id="r-save" style="flex:2">Save &amp; next</button>
          <button class="btn" id="r-skip">Skip</button>
        </div>
        <div class="row gap" style="margin-top:8px">
          <button class="btn tiny" id="r-reveal">Show in folder</button>
        </div>
        <div class="kbd-hint">
          <kbd>Enter</kbd> save &amp; next · <kbd>S</kbd> skip · <kbd>1</kbd>–<kbd>5</kbd> rating<br>
          <kbd>N</kbd> NSFW · <kbd>P</kbd> popping · <kbd>D</kbd> destruction · <kbd>F</kbd> deflation · <kbd>I</kbd> inflation
        </div>
      </aside>
    </div>`;

  const editor = createEditor(body, 'r', v);
  r.editor = editor;

  const videoEl = $('#r-video');
  playVideo(videoEl, $('#r-overlay'), v);
  loadThumbs(v, $('#r-thumbs'), videoEl);

  $('#r-save').onclick = () => saveAndNext();
  $('#r-skip').onclick = () => skipCurrent();
  $('#r-reveal').onclick = () => invoke('reveal', { id: v.id }).catch((err) => toast(String(err), 'error'));

  const sub = $('#rate-sub');
  if (sub) sub.textContent = `${state.stats.unreviewed ?? r.queue.length} unreviewed in the library`;
}

async function saveAndNext() {
  const r = state.rate;
  if (r.saving || !r.editor) return;
  const v = r.queue[r.index];
  if (!v) return;
  r.saving = true;
  const patch = r.editor.read();
  try {
    await invoke('save_video', { id: v.id, patch });
    if (patch.reviewed) {
      r.queue.splice(r.index, 1);
    } else {
      r.index += 1;
    }
    await refreshMeta();
    renderRateCurrent();
  } catch (err) {
    toast(`Save failed: ${err}`, 'error');
  } finally {
    r.saving = false;
  }
}

async function skipCurrent() {
  const r = state.rate;
  if (!r.editor) return;
  const v = r.queue[r.index];
  if (!v) return;
  const patch = { ...r.editor.read(), reviewed: false };
  try {
    await invoke('save_video', { id: v.id, patch });
  } catch (err) {
    /* editing-only save, ignore */
  }
  r.index += 1;
  refreshMeta();
  renderRateCurrent();
}

/* ----------------------------- duplicates -------------------------------- */

async function viewDuplicates() {
  $('#view').innerHTML = `
    <header class="page-head">
      <div>
        <h1>Duplicates</h1>
        <p class="muted" id="dup-sub">Looking for identical files…</p>
      </div>
      <div class="head-actions">
        <button class="btn" id="dup-refresh">Refresh</button>
      </div>
    </header>
    <div id="dup-body"><div class="empty">Comparing MD5 hashes…</div></div>`;
  $('#dup-refresh').onclick = () => viewDuplicates();

  try {
    state.dups = await invoke('duplicates');
  } catch (err) {
    toast(`Could not check duplicates: ${err}`, 'error');
    state.dups = [];
  }
  renderDups();
}

function renderDups() {
  const body = $('#dup-body');
  const sub = $('#dup-sub');
  if (!body) return;
  const groups = state.dups;
  const wasted = groups.reduce((sum, g) => sum + g.wasted, 0);
  if (sub) {
    sub.textContent = groups.length
      ? `${groups.length} duplicate group${groups.length === 1 ? '' : 's'} · ${fmtSize(wasted)} recoverable`
      : 'No byte-identical duplicates found.';
  }
  if (!groups.length) {
    const pending = state.stats.hashPending || 0;
    body.innerHTML = `
      <div class="empty">
        <h2>No duplicates</h2>
        <p>Every hashed video has a unique MD5 fingerprint.</p>
        ${pending ? `<p class="warn">${pending} file${pending === 1 ? '' : 's'} still need hashing — run a scan to finish.</p>` : ''}
        <p><button class="btn" id="dup-scan">Run a scan</button></p>
      </div>`;
    $('#dup-scan').onclick = startScan;
    return;
  }

  body.innerHTML = `<div class="dup-list">${groups.map(dupGroupHtml).join('')}</div>`;

  $$('.dup-group', body).forEach((groupEl) => {
    const md5 = groupEl.dataset.md5;
    const group = groups.find((g) => g.md5 === md5);
    if (!group) return;
    if (!state.keep[md5]) state.keep[md5] = autoPick(group.videos);

    const syncKept = () => {
      const keepId = state.keep[md5];
      $$('tbody tr', groupEl).forEach((tr) => {
        tr.classList.toggle('kept', Number(tr.dataset.id) === keepId);
        const radio = $('input[type="radio"]', tr);
        if (radio) radio.checked = Number(tr.dataset.id) === keepId;
      });
    };
    syncKept();

    $$('input[type="radio"]', groupEl).forEach((radio) => {
      radio.onchange = () => {
        state.keep[md5] = Number(radio.value);
        syncKept();
      };
    });
    $$('[data-reveal-id]', groupEl).forEach((b) => {
      b.onclick = () => invoke('reveal', { id: Number(b.dataset.revealId) })
        .catch((err) => toast(String(err), 'error'));
    });
    const auto = $('[data-auto-pick]', groupEl);
    if (auto) auto.onclick = () => { state.keep[md5] = autoPick(group.videos); syncKept(); };
    const trash = $('[data-trash]', groupEl);
    if (trash) trash.onclick = () => trashOthers(group);
  });
}

function autoPick(videos) {
  const sorted = videos.slice().sort((a, b) => {
    if (b.viewCount !== a.viewCount) return b.viewCount - a.viewCount;
    return a.addedAt - b.addedAt;
  });
  return sorted[0].id;
}

function dupGroupHtml(group) {
  return `
    <section class="dup-group" data-md5="${esc(group.md5)}">
      <div class="dup-head">
        <span class="hash">${esc(group.md5.slice(0, 16))}…</span>
        <span>${group.videos.length} copies</span>
        <span class="save">${fmtSize(group.wasted)} recoverable</span>
      </div>
      <table class="dup-table">
        <thead>
          <tr>
            <th style="width:34px">Keep</th>
            <th>File</th>
            <th style="width:90px">Size</th>
            <th style="width:120px">Resolution</th>
            <th style="width:80px">Duration</th>
            <th style="width:90px">Views</th>
            <th style="width:110px">Added</th>
            <th style="width:90px"></th>
          </tr>
        </thead>
        <tbody>
          ${group.videos.map((v) => `
            <tr data-id="${v.id}">
              <td><input type="radio" name="keep-${esc(group.md5)}" value="${v.id}" ${state.keep[group.md5] === v.id ? 'checked' : ''}></td>
              <td>
                <div class="fname" title="${esc(v.path)}">${esc(v.fileName)}</div>
                <div class="muted small fname" title="${esc(v.path)}">${esc(v.path)}</div>
              </td>
              <td>${fmtSize(v.size)}</td>
              <td>${v.width ? `${v.width}×${v.height}` : '—'}</td>
              <td>${v.durationMs ? fmtDur(v.durationMs) : '—'}</td>
              <td>${v.viewCount}</td>
              <td>${fmtDate(v.addedAt)}</td>
              <td><button class="btn tiny" data-reveal-id="${v.id}">Folder</button></td>
            </tr>`).join('')}
        </tbody>
      </table>
      <div class="dup-actions">
        <button class="btn tiny" data-auto-pick>Auto-pick keeper</button>
        <button class="btn danger" data-trash>Move others to trash</button>
      </div>
    </section>`;
}

async function trashOthers(group) {
  const keepId = state.keep[group.md5];
  const others = group.videos.filter((v) => v.id !== keepId);
  if (!others.length) return;
  const trash = state.boot?.trashDir || 'the library _trash folder';
  const ok = await confirmDialog({
    title: 'Move duplicates to trash?',
    message: `Keeping:\n${group.videos.find((v) => v.id === keepId)?.fileName}\n\n` +
      `Moving ${others.length} file${others.length === 1 ? '' : 's'} to:\n${trash}\n\n` +
      'They can be restored from that folder.',
    okLabel: 'Move to trash',
    danger: true,
  });
  if (!ok) return;

  let moved = 0;
  for (const v of others) {
    try {
      await invoke('remove_duplicate', { id: v.id });
      moved += 1;
    } catch (err) {
      toast(`Could not move ${v.fileName}: ${err}`, 'error');
    }
  }
  state.dups = state.dups.filter((g) => g.md5 !== group.md5);
  delete state.keep[group.md5];
  toast(`Moved ${moved} file${moved === 1 ? '' : 's'} to trash.`);
  renderDups();
  refreshMeta();
}

/* ------------------------------ settings -------------------------------- */

function scanPanelHtml(scan) {
  if (!scan || (!scan.running && !scan.phase)) {
    return '<p class="muted small" style="margin-top:14px">Idle.</p>';
  }
  if (scan.running) {
    const pct = scan.total ? Math.round((100 * scan.done) / scan.total) : 0;
    return `
      <div class="scan-box">
        <div class="row between">
          <strong>${esc(phaseLabel(scan.phase))}</strong>
          <span class="muted small">${scan.done} / ${scan.total}</span>
        </div>
        <div class="progress"><div style="width:${Math.max(3, pct)}%"></div></div>
        <div class="muted small ellipsis" title="${esc(scan.current)}">${esc(scan.current || '')}</div>
        <div class="muted small" style="margin-top:4px">${scan.added} new · ${scan.updated} changed · ${scan.failed} failed</div>
      </div>`;
  }
  const when = scan.finishedAt ? new Date(scan.finishedAt * 1000).toLocaleString() : '';
  const error = scan.error ? `<p class="warn small">${esc(scan.error)}</p>` : '';
  return `
    <div class="scan-box">
      <div class="row between"><strong>Last scan</strong><span class="muted small">${esc(when)}</span></div>
      <div class="muted small" style="margin-top:6px">
        ${scan.added} new · ${scan.updated} changed · ${scan.removed} missing · ${scan.failed} failed
      </div>
      ${error}
    </div>`;
}

function viewSettings() {
  const b = state.boot || {};
  const scan = state.scan || {};
  $('#view').innerHTML = `
    <header class="page-head">
      <div>
        <h1>Settings</h1>
        <p class="muted">Library folder, indexing and tooling</p>
      </div>
    </header>
    <div class="settings-grid">
      <section class="panel">
        <h2>Library folder</h2>
        <div class="path-row">
          <input class="input" readonly value="${esc(b.root || '')}" placeholder="No folder selected">
          <button class="btn" id="set-pick">Choose…</button>
        </div>
        <div class="row gap">
          <button class="btn primary" id="set-scan">Scan library</button>
          <button class="btn" id="set-cancel" ${scan.running ? '' : 'disabled'}>Stop scan</button>
        </div>
        <div id="scan-panel">${scanPanelHtml(scan)}</div>
        <p class="muted small" style="margin-top:12px">
          A scan walks the folder, reads duration/resolution/codecs with ffprobe,
          computes MD5 hashes for duplicate detection and marks deleted files as missing.
          New videos appear in the Rate queue.
        </p>
      </section>

      <section class="panel">
        <h2>ffmpeg</h2>
        ${b.ffmpegOk
          ? `<div class="kv">Ready. Using <code>${esc(b.ffmpegDir || '')}</code></div>`
          : `<p class="warn small" style="margin-top:0">Not found. Thumbnails and playback of MKV/FLV/WMV need ffmpeg.</p>
             <p class="muted small">Install with <code>winget install Gyan.FFmpeg</code> or point to the folder that contains ffmpeg.exe and ffprobe.exe.</p>`}
        <div class="path-row" style="margin-top:10px">
          <input class="input" id="set-ffmpeg" placeholder="C:\\path\\to\\ffmpeg\\bin" value="${esc(b.ffmpegDir || '')}">
          <button class="btn" id="set-ffmpeg-save">Apply</button>
        </div>
      </section>

      <section class="panel">
        <h2>Storage</h2>
        <div class="kv">Database folder: <code>${esc(b.dataDir || '')}</code></div>
        <div class="kv">Library file: <code>${esc(b.dbPath || '')}</code></div>
        <div class="row gap" style="margin:8px 0 10px">
          <button class="btn" id="set-db-dir">Change folder…</button>
          ${b.dataDir && b.defaultDataDir && b.dataDir !== b.defaultDataDir
            ? '<button class="btn" id="set-db-default">Use default</button>' : ''}
        </div>
        ${b.dataWarning ? `<p class="warn small">${esc(b.dataWarning)}</p>` : ''}
        <p class="muted small">Moving copies <code>library.db</code> to the chosen folder; the current file stays behind as a backup. An existing <code>library.db</code> in the target folder is renamed aside first.</p>
        <div class="kv" style="margin-top:12px">Cache: <code>${esc(b.cacheDir || '')}</code> — <span id="cache-size">…</span></div>
        <div class="kv">Trash: <code>${esc(b.trashDir || '—')}</code></div>
        <div class="row gap" style="margin-top:10px">
          <button class="btn" id="set-clear-cache">Clear cache</button>
          <button class="btn" id="set-refresh-cache">Recalculate size</button>
        </div>
        <p class="muted small">The cache holds generated preview frames and converted MP4s; it is rebuilt on demand.</p>
      </section>

      <section class="panel">
        <h2>Library stats</h2>
        <div class="kv">Videos: ${state.stats.total || 0} (${fmtSize(state.stats.totalSize || 0)})</div>
        <div class="kv">Reviewed: ${state.stats.reviewed || 0} · Unreviewed: ${state.stats.unreviewed || 0}</div>
        <div class="kv">Tagged: ${state.stats.tagged || 0} · NSFW: ${state.stats.nsfw || 0}</div>
        <div class="kv">Duplicate groups: ${state.stats.duplicates || 0}</div>
        <div class="kv">Awaiting hash: ${state.stats.hashPending || 0}</div>
        <div class="kv">Total duration: ${fmtDur(state.stats.totalDurationMs || 0)}</div>
      </section>

      <section class="panel">
        <h2>Shortcuts (Rate view)</h2>
        <ul class="shortcut-list">
          <li><span><kbd>Enter</kbd></span><span>Save &amp; next</span></li>
          <li><span><kbd>S</kbd></span><span>Skip (keeps unreviewed)</span></li>
          <li><span><kbd>1</kbd>–<kbd>5</kbd></span><span>Set star rating</span></li>
          <li><span><kbd>N</kbd> <kbd>P</kbd> <kbd>D</kbd> <kbd>F</kbd> <kbd>I</kbd></span><span>Toggle flags</span></li>
          <li><span><kbd>Esc</kbd></span><span>Close dialog</span></li>
        </ul>
      </section>

      <section class="panel">
        <h2>About</h2>
        <div class="kv">Splashhound ${esc(b.version || '')}</div>
        <div class="kv">Local-only app. No network access beyond 127.0.0.1 media streaming.</div>
        <div class="kv">Media cache is served on <code>${esc(b.mediaBase || '')}</code></div>
      </section>
    </div>`;

  $('#set-pick').onclick = async () => {
    try {
      const path = await invoke('pick_folder');
      if (!path) return;
      const saved = await invoke('set_root', { path });
      state.boot.root = saved;
      state.boot.trashDir = `${saved}\\_trash`;
      toast('Library folder set.');
      startScan();
      viewSettings();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $('#set-scan').onclick = startScan;

  $('#set-cancel').onclick = async () => {
    try {
      await invoke('cancel_scan');
      toast('Stopping scan…');
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $('#set-ffmpeg-save').onclick = async () => {
    const path = $('#set-ffmpeg').value.trim();
    try {
      const ok = await invoke('set_ffmpeg_dir', { path: path || null });
      state.boot.ffmpegOk = ok;
      state.boot.ffmpegDir = path || null;
      toast(ok ? 'ffmpeg found.' : 'ffmpeg not found at that path.', ok ? 'info' : 'error');
      viewSettings();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  const loadCacheSize = async () => {
    try {
      const bytes = await invoke('cache_size');
      const el = $('#cache-size');
      if (el) el.textContent = fmtSize(bytes);
    } catch (err) {
      /* ignore */
    }
  };
  loadCacheSize();

  $('#set-refresh-cache').onclick = loadCacheSize;
  const moveDatabase = async (target) => {
    try {
      const res = await invoke('set_data_dir', { path: target });
      state.boot.dataDir = res.dataDir;
      state.boot.dbPath = res.dbPath;
      state.boot.dataWarning = null;
      toast(res.backup
        ? `Database moved. The previous file in the target folder was kept as ${res.backup}`
        : 'Database moved.');
      await refreshMeta();
      viewSettings();
    } catch (err) {
      toast(`Could not move the database: ${err}`, 'error');
    }
  };

  $('#set-db-dir').onclick = async () => {
    let path;
    try {
      path = await invoke('pick_folder');
    } catch (err) {
      toast(String(err), 'error');
      return;
    }
    if (!path) return;
    const ok = await confirmDialog({
      title: 'Move the library database?',
      message: `Copy library.db to:\n${path}\n\nThe current file stays where it is as a backup.`,
      okLabel: 'Move database',
    });
    if (ok) await moveDatabase(path);
  };

  const defaultDbBtn = $('#set-db-default');
  if (defaultDbBtn) {
    defaultDbBtn.onclick = async () => {
      const ok = await confirmDialog({
        title: 'Move the database back?',
        message: `Copy library.db to the default folder:\n${state.boot.defaultDataDir}\n\nThe current file stays where it is as a backup.`,
        okLabel: 'Use default folder',
      });
      if (ok) await moveDatabase(null);
    };
  }

  $('#set-clear-cache').onclick = async () => {
    const ok = await confirmDialog({
      title: 'Clear generated cache?',
      message: 'Thumbnails and converted playback files will be regenerated when needed.',
      okLabel: 'Clear cache',
    });
    if (!ok) return;
    try {
      await invoke('clear_cache');
      toast('Cache cleared.');
      loadCacheSize();
    } catch (err) {
      toast(String(err), 'error');
    }
  };
}

/* ---------------------------- scan polling ------------------------------- */

async function startScan() {
  try {
    await invoke('start_scan');
    toast('Scan started.');
    startScanPolling();
    if (state.view === 'settings') updateScanUI();
  } catch (err) {
    toast(String(err), 'error');
    if (!state.boot?.root) navigate('settings');
  }
}

function startScanPolling() {
  if (state.scanTimer) clearInterval(state.scanTimer);
  pollScan();
  state.scanTimer = setInterval(pollScan, 900);
}

async function pollScan() {
  try {
    const next = await invoke('scan_status');
    const wasRunning = !!state.scan?.running;
    state.scan = next;
    updateScanUI();
    if (wasRunning && !next.running) {
      if (state.scanTimer) clearInterval(state.scanTimer);
      state.scanTimer = null;
      await refreshMeta();
      if (next.error) toast(next.error, 'error');
      else toast(`Scan finished — ${next.added} new, ${next.updated} changed, ${next.removed} missing.`);
      rerenderView();
      if (state.view === 'settings') renderScanPanelOnly();
    }
  } catch (err) {
    if (state.scanTimer) clearInterval(state.scanTimer);
    state.scanTimer = null;
  }
}

function updateScanUI() {
  const s = state.scan;
  const line = $('#side-scan');
  if (line) {
    if (s?.running) {
      const pct = s.total ? Math.round((100 * s.done) / s.total) : 0;
      line.textContent = `${phaseLabel(s.phase)} ${pct}%`;
    } else if (s?.error) {
      line.textContent = 'Scan error — see Settings';
    } else {
      line.textContent = '';
    }
  }
  const cancel = $('#set-cancel');
  if (cancel) cancel.disabled = !s?.running;
  renderScanPanelOnly();
}

function renderScanPanelOnly() {
  const panel = $('#scan-panel');
  if (panel) panel.innerHTML = scanPanelHtml(state.scan);
  const scanBtn = $('#set-scan');
  if (scanBtn) scanBtn.disabled = !!state.scan?.running;
}

/* --------------------------- keyboard shortcuts --------------------------- */

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && modalCloser) {
    modalCloser();
    return;
  }
  if (modalCloser) return;
  if (state.view !== 'rate' || !state.rate.editor) return;
  const target = e.target;
  const tag = (target?.tagName || '').toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select' || tag === 'video' || target?.isContentEditable) {
    return;
  }
  const key = e.key.toLowerCase();
  if (e.key === 'Enter') { e.preventDefault(); saveAndNext(); return; }
  if (key === 's') { e.preventDefault(); skipCurrent(); return; }
  if (/^[1-5]$/.test(key)) { e.preventDefault(); state.rate.editor.setRating(Number(key)); return; }
  const flagKeys = { n: 'nsfw', p: 'popping', d: 'destruction', f: 'deflation', i: 'inflation' };
  if (flagKeys[key]) { e.preventDefault(); state.rate.editor.toggleFlag(flagKeys[key]); }
});
