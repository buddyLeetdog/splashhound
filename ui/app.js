'use strict';

/* ---------------------------------------------------------------------------
 * Splashhound frontend - plain JS against the Tauri command bridge.
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
  folders: [],
  scan: null,
  scanTimer: null,
  view: '',
  playerToken: 0,
  lib: { filter: null, items: [], total: 0, loading: false, selectMode: false, selected: new Set() },
  rate: { queue: [], index: 0, loading: false, saving: false, editor: null, video: null, folder: '' },
  dups: [],
  similar: [],
  dupsTab: 'exact',
  trash: [],
  trashSelected: new Set(),
  detail: null,
  dnaTimer: null,
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
  if (!unix) return '-';
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
  $('#count-trash').textContent = s.trash ?? 0;
  const pct = s.total ? Math.round((100 * (s.reviewed || 0)) / s.total) : 0;
  $('#side-progress').style.width = `${pct}%`;
  $('#side-progress-text').textContent = `${s.reviewed || 0} / ${s.total || 0} reviewed`;
  updateScanUI();
}

async function refreshMeta() {
  try {
    const [stats, tags, toys, creators, folders] = await Promise.all([
      invoke('stats'), invoke('all_tags'), invoke('all_toys'), invoke('creators'), invoke('folders'),
    ]);
    state.stats = stats;
    state.tags = tags;
    state.toys = toys;
    state.creators = creators;
    state.folders = folders;
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
    let onKey = null;
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
          const okButton = $('#dlg-ok', root);
          // Enter confirms, Backspace cancels. Escape is handled by the global
          // modal handler, which runs onClose below.
          onKey = (e) => {
            if (e.key === 'Enter' || e.key === 'NumpadEnter') {
              e.preventDefault();
              finish(true);
              close();
            } else if (e.key === 'Backspace') {
              e.preventDefault();
              finish(false);
              close();
            }
          };
          document.addEventListener('keydown', onKey);
          okButton.onclick = () => { finish(true); close(); };
          okButton.focus();
          $$('[data-close]', root).forEach((b) => b.addEventListener('click', () => finish(false)));
          $('.modal-backdrop', root).addEventListener('mousedown', (e) => {
            if (e.target.classList.contains('modal-backdrop')) finish(false);
          });
        },
        onClose() {
          if (onKey) document.removeEventListener('keydown', onKey);
          finish(false);
        },
      },
    );
  });
}

function promptDialog(title, label, initial = '') {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (!settled) {
        settled = true;
        resolve(value);
      }
    };
    modal(
      `<h2>${esc(title)}</h2>
       <div class="field">
         <label>${esc(label)}</label>
         <input class="input" id="prompt-input" value="${esc(initial)}">
       </div>
       <div class="modal-actions">
         <button class="btn" data-close>Cancel</button>
         <button class="btn primary" id="prompt-ok">OK</button>
       </div>`,
      {
        onMount(root, close) {
          const input = $('#prompt-input', root);
          input.focus();
          input.select();
          const submit = () => {
            const value = input.value.trim();
            if (!value) return;
            finish(value);
            close();
          };
          $('#prompt-ok', root).onclick = submit;
          input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              submit();
            } else if (e.key === 'Backspace' && !input.value) {
              // Empty field: Backspace cancels instead of doing nothing.
              e.preventDefault();
              finish(null);
              close();
            }
          });
          $$('[data-close]', root).forEach((b) => b.addEventListener('click', () => finish(null)));
          $('.modal-backdrop', root).addEventListener('mousedown', (e) => {
            if (e.target.classList.contains('modal-backdrop')) finish(null);
          });
        },
        onClose() {
          finish(null);
        },
      },
    );
  });
}

/* ------------------------------- init ----------------------------------- */
window.addEventListener('DOMContentLoaded', init);

async function init() {
  $$('.nav-item').forEach((b) => b.addEventListener('click', () => navigate(b.dataset.view)));
  const main = $('#main');
  main.addEventListener('scroll', maybeLoadMoreLibrary, { passive: true });
  main.addEventListener('wheel', (e) => {
    if (e.deltaY > 0) maybeLoadMoreLibrary();
  }, { passive: true });
  try {
    const boot = await invoke('bootstrap');
    state.boot = boot;
    state.media = boot.mediaBase;
    state.stats = boot.stats;
    state.tags = boot.tags;
    state.creators = boot.creators;
    state.scan = boot.scan;
    refreshChrome();
    await refreshMeta();
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
  else if (view === 'trash') viewTrash();
  else if (view === 'settings') viewSettings();
}

function rerenderView() {
  if (state.view === 'library') loadLibrary(true);
  else if (state.view === 'rate') loadRateQueue();
  else if (state.view === 'duplicates') viewDuplicates();
  else if (state.view === 'trash') viewTrash();
  else if (state.view === 'settings') viewSettings();
}

/* ------------------------------ library --------------------------------- */

function defaultFilter() {
  return {
    search: '',
    tags: [],
    tagMode: 'any',
    toys: [],
    nsfw: 'any',
    reviewed: 'any',
    minRating: 0,
    creator: '',
    folder: '',
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
        <p class="muted" id="lib-sub">Loading...</p>
      </div>
      <div class="head-actions">
        <button class="btn hidden" id="lib-rate-folder">Rate this folder</button>
        <button class="btn" id="lib-reshuffle">Shuffle</button>
        <button class="btn" id="lib-refresh">Refresh</button>
        <button class="btn primary" id="lib-scan">Scan folder</button>
      </div>
    </header>

    <div class="toolbar">
      <input class="input search" id="f-search" placeholder="Search file name, path, creator, notes..." value="${esc(f.search)}">
      <select class="select" id="f-folder">${folderOptionsHtml(f.folder)}</select>
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
        <option value="1">\u2605 1+</option>
        <option value="2">\u2605 2+</option>
        <option value="3">\u2605 3+</option>
        <option value="4">\u2605 4+</option>
        <option value="5">\u2605 5</option>
      </select>
      <select class="select" id="f-creator">${creatorOptions}</select>
      <select class="select" id="f-sort">
        <option value="added_desc">Newest added</option>
        <option value="name_asc">Name A \u2192 Z</option>
        <option value="rating_desc">Rating high \u2192 low</option>
        <option value="views_desc">Most viewed</option>
        <option value="duration_desc">Longest</option>
        <option value="size_desc">Largest file</option>
        <option value="random">Random</option>
      </select>
      <button class="btn" id="f-tags-btn">Tags <span class="pill" id="tag-count">0</span></button>
      <button class="btn" id="f-toys-btn">Toys <span class="pill" id="toy-count">0</span></button>
      <button class="btn" id="f-select">Select</button>
    </div>

    <div class="tag-filter hidden" id="tag-filter"></div>
    <div class="tag-filter hidden" id="toy-filter"></div>
    <div class="chips" id="active-filters" style="margin-bottom:12px"></div>

    <div class="bulk-bar hidden" id="bulk-bar">
      <span class="small" id="bulk-count">0 selected</span>
      <button class="btn tiny" id="bulk-all">Select all loaded</button>
      <button class="btn tiny" id="bulk-clear">Clear</button>
      <button class="btn primary tiny" id="bulk-edit">Edit selection...</button>
    </div>

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
  $('#f-folder').addEventListener('change', (e) => {
    f.folder = e.target.value;
    updateRateFolderButton();
    loadLibrary(true);
  });
  $('#f-sort').addEventListener('change', (e) => { f.sort = e.target.value; loadLibrary(true); });

  $('#f-tags-btn').addEventListener('click', () => {
    $('#tag-filter').classList.toggle('hidden');
  });
  $('#f-toys-btn').addEventListener('click', () => {
    $('#toy-filter').classList.toggle('hidden');
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
    if (e.target.closest('.card-check')) return;
    const card = e.target.closest('.card');
    if (!card) return;
    const id = Number(card.dataset.id);
    if (state.lib.selectMode) toggleSelection(id, !state.lib.selected.has(id));
    else openDetail(id);
  });
  grid.addEventListener('change', (e) => {
    const box = e.target.closest('.card-check input');
    if (box) toggleSelection(Number(box.dataset.id), box.checked);
  });

  $('#f-select').addEventListener('click', () => {
    state.lib.selectMode = !state.lib.selectMode;
    state.lib.selected.clear();
    const btn = $('#f-select');
    btn.classList.toggle('primary', state.lib.selectMode);
    btn.textContent = state.lib.selectMode ? 'Done' : 'Select';
    if (grid) grid.classList.toggle('selecting', state.lib.selectMode);
    renderLibraryGrid();
    updateBulkBar();
  });
  $('#bulk-all').addEventListener('click', () => {
    state.lib.items.forEach((v) => state.lib.selected.add(v.id));
    renderLibraryGrid();
    updateBulkBar();
  });
  $('#bulk-clear').addEventListener('click', () => {
    state.lib.selected.clear();
    renderLibraryGrid();
    updateBulkBar();
  });
  $('#bulk-edit').addEventListener('click', openBulkEditor);
  $('#lib-rate-folder').addEventListener('click', () => {
    state.rate.folder = f.folder;
    navigate('rate');
  });

  $('#tag-count').textContent = f.tags.length;
  $('#toy-count').textContent = f.toys.length;
  $('#f-select').classList.toggle('primary', state.lib.selectMode);
  $('#f-select').textContent = state.lib.selectMode ? 'Done' : 'Select';
  grid.classList.toggle('selecting', state.lib.selectMode);
  updateRateFolderButton();
  updateBulkBar();
  renderTagFilter();
  renderToyFilter();
  renderActiveFilters();
  loadLibrary(true);
}

/* ------------------------------ folders -------------------------------- */

function folderLabel(path) {
  if (!path) return '';
  const root = (state.boot?.root || '').replace(/\\+$/, '');
  if (root && path.toLowerCase().startsWith(`${root.toLowerCase()}\\`)) {
    return path.slice(root.length + 1);
  }
  return path;
}

function folderOptionsHtml(selected, allLabel = 'All folders') {
  const options = [`<option value="">${esc(allLabel)}</option>`];
  state.folders.forEach((folder) => {
    options.push(
      `<option value="${esc(folder.path)}" ${folder.path === selected ? 'selected' : ''}>${esc(folderLabel(folder.path))} (${folder.count})</option>`,
    );
  });
  return options.join('');
}

function updateRateFolderButton() {
  const btn = $('#lib-rate-folder');
  if (!btn) return;
  const folder = state.lib.filter?.folder;
  btn.classList.toggle('hidden', !folder);
  if (folder) btn.title = `Rate only ${folderLabel(folder)}`;
}

function toggleSelection(id, on) {
  if (on) state.lib.selected.add(id);
  else state.lib.selected.delete(id);
  const card = $(`.card[data-id="${id}"]`);
  if (card) {
    card.classList.toggle('selected', on);
    const box = $('.card-check input', card);
    if (box) box.checked = on;
  }
  updateBulkBar();
}

function updateBulkBar() {
  const bar = $('#bulk-bar');
  if (!bar) return;
  const count = state.lib.selected.size;
  bar.classList.toggle('hidden', count === 0);
  const label = $('#bulk-count');
  if (label) label.textContent = `${count} selected`;
}

async function openBulkEditor() {
  const ids = Array.from(state.lib.selected);
  if (!ids.length) return;
  closeAutocomplete();
  modal(`
    <h2>Edit ${ids.length} video${ids.length === 1 ? '' : 's'}</h2>
    <div class="bulk-grid">
      <div class="field">
        <label>Creator</label>
        <input class="input" id="b-creator" placeholder="Leave unchanged">
        <label class="check" style="margin-top:6px"><input type="checkbox" id="b-clear-creator"> Clear creator</label>
      </div>
      <div class="field">
        <label>Rating</label>
        <select class="select" id="b-rating">
          <option value="">Leave unchanged</option>
          <option value="0">No rating</option>
          <option value="1">\u2605 1</option><option value="2">\u2605 2</option><option value="3">\u2605 3</option>
          <option value="4">\u2605 4</option><option value="5">\u2605 5</option>
        </select>
      </div>
      <div class="field">
        <label>Reviewed</label>
        <select class="select" id="b-reviewed">
          <option value="">Leave unchanged</option>
          <option value="1">Reviewed</option>
          <option value="0">Unreviewed</option>
        </select>
      </div>
      <div class="field">
        <label>Flags</label>
        <div class="bulk-flags">
          ${FLAGS.map(([k, label]) => `
            <label class="check bulk-flag"><span>${label}</span>
              <select class="select" data-bulk-flag="${k}">
                <option value="">-</option><option value="1">yes</option><option value="0">no</option>
              </select>
            </label>`).join('')}
        </div>
      </div>
      <div class="field">
        <label>Add tags</label>
        <input class="input" id="b-add-tags" placeholder="comma separated">
      </div>
      <div class="field">
        <label>Remove tags</label>
        <input class="input" id="b-remove-tags" placeholder="comma separated">
      </div>
      <div class="field">
        <label>Add toys</label>
        <input class="input" id="b-add-toys" placeholder="comma separated">
      </div>
      <div class="field">
        <label>Remove toys</label>
        <input class="input" id="b-remove-toys" placeholder="comma separated">
      </div>
    </div>
    <div class="modal-actions">
      <button class="btn" data-close>Cancel</button>
      <button class="btn primary" id="b-apply">Apply</button>
    </div>`, {
    onMount(root, close) {
      $('#b-apply', root).onclick = async () => {
        const edit = {
          ids,
          addTags: $('#b-add-tags', root).value.split(',').map((s) => s.trim()).filter(Boolean),
          removeTags: $('#b-remove-tags', root).value.split(',').map((s) => s.trim()).filter(Boolean),
          addToys: $('#b-add-toys', root).value.split(',').map((s) => s.trim()).filter(Boolean),
          removeToys: $('#b-remove-toys', root).value.split(',').map((s) => s.trim()).filter(Boolean),
        };
        const creator = $('#b-creator', root).value.trim();
        if ($('#b-clear-creator', root).checked) edit.creator = '';
        else if (creator) edit.creator = creator;
        const rating = $('#b-rating', root).value;
        if (rating !== '') edit.rating = Number(rating);
        const reviewed = $('#b-reviewed', root).value;
        if (reviewed !== '') edit.reviewed = reviewed === '1';
        $$('[data-bulk-flag]', root).forEach((select) => {
          if (select.value !== '') edit[select.dataset.bulkFlag] = select.value === '1';
        });
        try {
          const updated = await invoke('bulk_update', { edit });
          toast(`Updated ${updated} video${updated === 1 ? '' : 's'}.`);
          close();
          state.lib.selected.clear();
          updateBulkBar();
          await refreshMeta();
          loadLibrary(true);
        } catch (err) {
          toast(`Bulk edit failed: ${err}`, 'error');
        }
      };
    },
  });
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
        : '<span class="muted small">No tags yet - add them while rating videos.</span>'}
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
      <span class="chip active">${esc(t)}<button class="chip-x" data-remove="${esc(t)}">\u00d7</button></span>`).join('')
    + f.toys.map((t) => `
      <span class="chip active toy">${esc(t)}<button class="chip-x" data-remove-toy="${esc(t)}">\u00d7</button></span>`).join('');
  $$('[data-remove]', wrap).forEach((b) => {
    b.onclick = () => {
      f.tags = f.tags.filter((t) => t !== b.dataset.remove);
      $('#tag-count').textContent = f.tags.length;
      renderTagFilter();
      renderActiveFilters();
      loadLibrary(true);
    };
  });
  $$('[data-remove-toy]', wrap).forEach((b) => {
    b.onclick = () => {
      f.toys = f.toys.filter((t) => t !== b.dataset.removeToy);
      $('#toy-count').textContent = f.toys.length;
      renderToyFilter();
      renderActiveFilters();
      loadLibrary(true);
    };
  });
}

function renderToyFilter() {
  const panel = $('#toy-filter');
  if (!panel) return;
  const f = state.lib.filter;
  panel.innerHTML = `
    <div class="tag-filter-head">
      <strong>Filter by toy</strong>
      <button class="btn tiny" id="toy-clear">Clear</button>
    </div>
    <div class="chips">
      ${state.toys.length
        ? state.toys.map((t) => `<button class="chip ${f.toys.includes(t.name) ? 'active' : ''}" data-toy="${esc(t.name)}">${esc(t.name)} <span class="n">${t.count}</span></button>`).join('')
        : '<span class="muted small">No toys yet - add them while rating videos.</span>'}
    </div>`;
  $$('[data-toy]', panel).forEach((btn) => {
    btn.onclick = () => {
      const name = btn.dataset.toy;
      const idx = f.toys.indexOf(name);
      if (idx >= 0) f.toys.splice(idx, 1);
      else f.toys.push(name);
      $('#toy-count').textContent = f.toys.length;
      renderToyFilter();
      renderActiveFilters();
      loadLibrary(true);
    };
  });
  $('#toy-clear', panel).onclick = () => {
    f.toys = [];
    $('#toy-count').textContent = '0';
    renderToyFilter();
    renderActiveFilters();
    loadLibrary(true);
  };
}

function libraryCanLoadMore() {
  if (state.view !== 'library' || !state.lib.filter) return false;
  if (state.lib.loading) return false;
  if (state.lib.filter.sort === 'random') return false;
  return state.lib.items.length > 0 && state.lib.items.length < state.lib.total;
}

/**
 * Infinite scroll for the library grid: when the user reaches (or tries to
 * scroll past) the bottom, pull the next page if there is one.
 */
function maybeLoadMoreLibrary() {
  if (!libraryCanLoadMore()) return;
  const main = $('#main');
  if (!main) return;
  const remaining = main.scrollHeight - main.scrollTop - main.clientHeight;
  if (remaining < 480) loadLibrary(false);
}

async function loadLibrary(reset) {
  const grid = $('#lib-grid');
  if (!grid) return;
  if (state.lib.loading) return;
  const f = state.lib.filter;
  const moreButton = $('#lib-more');
  if (reset) {
    state.lib.items = [];
    f.offset = 0;
    grid.innerHTML = '';
  } else if (moreButton) {
    moreButton.disabled = true;
    moreButton.textContent = 'Loading…';
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
    if (moreButton) {
      moreButton.disabled = false;
      moreButton.textContent = 'Load more';
    }
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
      || state.lib.filter.reviewed !== 'any' || state.lib.filter.minRating || state.lib.filter.creator
      || state.lib.filter.folder || state.lib.filter.toys.length;
    sub.textContent = filtered
      ? `${items.length} of ${total} matching video${total === 1 ? '' : 's'}`
      : `${total} video${total === 1 ? '' : 's'} \u00b7 ${fmtSize(state.stats.totalSize || 0)}`;
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
  const toyBadges = (v.toys || [])
    .map((t) => `<span class="badge toy">${esc(t)}</span>`)
    .join('');
  const res = v.width ? `${v.width}\u00d7${v.height}` : '';
  const selected = state.lib.selected.has(v.id);
  return `
    <article class="card ${selected ? 'selected' : ''}" data-id="${v.id}">
      <div class="poster">
        <img loading="lazy" src="${state.media}/poster/${v.id}?k=${v.mtime}" alt="">
        <span class="dur">${v.durationMs ? fmtDur(v.durationMs) : '-'}</span>
        ${v.rating ? `<span class="rating-badge">\u2605 ${v.rating}</span>` : ''}
        ${v.reviewed ? '' : '<span class="new-badge">new</span>'}
        ${state.lib.selectMode ? `<label class="card-check"><input type="checkbox" data-id="${v.id}" ${selected ? 'checked' : ''}></label>` : ''}
      </div>
      <div class="card-body">
        <div class="card-title" title="${esc(v.fileName)}">${esc(v.fileName)}</div>
        <div class="card-meta">${[res, fmtSize(v.size), v.creator].filter(Boolean).join(' \u00b7 ')}</div>
        ${badges || toyBadges ? `<div class="badge-row">${badges}${toyBadges}</div>` : ''}
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
          ${[1, 2, 3, 4, 5].map((n) => `<button type="button" data-star="${n}" title="${n} star${n > 1 ? 's' : ''}">\u2605</button>`).join('')}
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
      <label>Toys</label>
      <div class="chips chips-static" id="${prefix}-toychips"></div>
      <input class="input" id="${prefix}-toyinput" placeholder="Add a toy and press Enter" style="margin-top:6px">
    </div>
    <div class="field">
      <label>Notes</label>
      <textarea class="input" id="${prefix}-notes" rows="3" placeholder="Optional notes...">${esc(v.notes || '')}</textarea>
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
    toys: (video.toys || []).slice(),
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
      ? form.tags.map((t, i) => `<span class="chip">${esc(t)}<button class="chip-x" data-tag-i="${i}" title="Remove">\u00d7</button></span>`).join('')
      : '<span class="muted small">No tags</span>';
  };

  const renderToys = () => {
    const wrap = $(`#${prefix}-toychips`, root);
    if (!wrap) return;
    wrap.innerHTML = form.toys.length
      ? form.toys.map((t, i) => `<span class="chip toy">${esc(t)}<button class="chip-x" data-toy-i="${i}" title="Remove">\u00d7</button></span>`).join('')
      : '<span class="muted small">No toys</span>';
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

  const toyInput = $(`#${prefix}-toyinput`, root);
  const addToy = (raw) => {
    const name = String(raw || '').trim().replace(/,+$/, '');
    if (!name) return;
    if (!form.toys.some((t) => t.toLowerCase() === name.toLowerCase())) {
      form.toys.push(name);
      renderToys();
    }
    toyInput.value = '';
  };
  attachAutocomplete(toyInput, (query) => {
    const q = query.toLowerCase();
    const taken = new Set(form.toys.map((t) => t.toLowerCase()));
    const matches = state.toys
      .filter((t) => !taken.has(t.name.toLowerCase()) && (!q || t.name.toLowerCase().includes(q)))
      .sort((a, b) => {
        const as = a.name.toLowerCase().startsWith(q) ? 0 : 1;
        const bs = b.name.toLowerCase().startsWith(q) ? 0 : 1;
        if (as !== bs) return as - bs;
        if (b.count !== a.count) return b.count - a.count;
        return a.name.localeCompare(b.name);
      })
      .map((t) => ({ value: t.name, meta: t.count }));
    if (q && !state.toys.some((t) => t.name.toLowerCase() === q) && !taken.has(q)) {
      matches.unshift({ value: query, meta: 'new', create: true });
    }
    return matches;
  }, { onPick: addToy });
  toyInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      e.stopPropagation();
      addToy(toyInput.value);
    } else if (e.key === 'Backspace' && !toyInput.value && form.toys.length) {
      form.toys.pop();
      renderToys();
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
    const toyBtn = e.target.closest('[data-toy-i]');
    if (toyBtn) {
      form.toys.splice(Number(toyBtn.dataset.toyI), 1);
      renderToys();
    }
  });

  renderStars();
  renderTags();
  renderToys();

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
        toys: form.toys.slice(),
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
  overlay.innerHTML = '<div class="spinner"></div><div>Preparing...</div>';

  (async () => {
    try {
      let info = await invoke('prepare_playback', { id: video.id });
      while (token === state.playerToken && (info.state === 'working' || info.state === 'queued')) {
        const pct = info.progress >= 0 ? Math.round(info.progress * 100) : null;
        overlay.innerHTML = `
          <div class="spinner"></div>
          <div>Converting for playback... ${pct === null ? '' : pct + '%'}</div>
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

async function openDetail(id, options = {}) {
  let v;
  try {
    v = await invoke('get_video', { id });
  } catch (err) {
    toast(String(err), 'error');
    return;
  }
  if (!v) return;

  let ids = (options.ids && options.ids.length
    ? options.ids
    : state.lib.items.map((item) => item.id)).slice();
  let index = options.index;
  if (index === undefined) index = ids.indexOf(id);
  if (index < 0 || !ids.includes(id)) {
    ids = [id];
    index = 0;
  }

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
        <div class="row between" style="margin-bottom:6px">
          <span class="muted small">${index + 1} / ${ids.length}</span>
          <div class="row gap">
            <button class="btn tiny" id="d-prev" ${index <= 0 ? 'disabled' : ''}>Prev</button>
            <button class="btn tiny" id="d-next" ${index >= ids.length - 1 ? 'disabled' : ''}>Next</button>
          </div>
        </div>
        <h2 class="detail-title">${esc(v.fileName)}</h2>
        <div class="muted small">${esc(v.path)}</div>
        <dl class="meta-table">
          <dt>Duration</dt><dd>${v.durationMs ? fmtDur(v.durationMs) : '-'}</dd>
          <dt>Size</dt><dd>${fmtSize(v.size)}</dd>
          <dt>Resolution</dt><dd>${v.width ? `${v.width}\u00d7${v.height}` : '-'}</dd>
          <dt>Codecs</dt><dd>${esc(v.vcodec || '?')} / ${esc(v.acodec || 'no audio')}</dd>
          <dt>Playback</dt><dd>${esc(v.playback)}</dd>
          <dt>MD5</dt><dd class="mono">${esc(v.md5 || 'not hashed yet')}</dd>
          <dt>Views</dt><dd>${v.viewCount}</dd>
          <dt>Added</dt><dd>${fmtDate(v.addedAt)}</dd>
        </dl>
        ${editorHtml(v, 'd')}
        <div class="modal-actions">
          <button class="btn" id="d-reveal">Show in folder</button>
          <button class="btn danger" id="d-delete">Move to trash</button>
          <button class="btn" data-close>Close</button>
          <button class="btn primary" id="d-save">Save</button>
        </div>
      </div>
    </div>`, {
    className: 'wide',
    onClose() {
      state.detail = null;
      state.playerToken += 1;
    },
    onMount(root, close) {
      const editor = createEditor(root, 'd', v);
      const videoEl = $('#d-video', root);
      playVideo(videoEl, $('#d-overlay', root), v);
      loadThumbs(v, $('#d-thumbs', root), videoEl);

      const persist = async () => {
        await invoke('save_video', { id: v.id, patch: editor.read() });
      };
      state.detail = { ids, index, editor, close, persist, video: v };

      $('#d-prev', root).onclick = () => navigateDetail(-1);
      $('#d-next', root).onclick = () => navigateDetail(1);
      $('#d-reveal', root).onclick = () => {
        invoke('reveal', { id: v.id }).catch((err) => toast(String(err), 'error'));
      };
      $('#d-delete', root).onclick = async () => {
        const ok = await confirmDialog({
          title: 'Move this video to trash?',
          message: `${v.fileName}\n\nIt will be moved to ${state.boot?.trashDir || 'the _trash folder'} and can be restored from the Trash view.`,
          okLabel: 'Move to trash',
          danger: true,
        });
        if (!ok) {
          // The confirm dialog replaced this modal; bring it back.
          openDetail(v.id, { ids, index });
          return;
        }
        try {
          await invoke('trash_video', { id: v.id });
          state.detail = null;
          close();
          toast('Moved to trash.');
          await refreshMeta();
          loadLibrary(true);
        } catch (err) {
          toast(String(err), 'error');
        }
      };
      $('#d-save', root).onclick = async () => {
        try {
          await persist();
          toast('Saved.');
          refreshMeta();
        } catch (err) {
          toast(`Save failed: ${err}`, 'error');
        }
      };
    },
  });
}

async function navigateDetail(delta) {
  const detail = state.detail;
  if (!detail) return;
  let index = detail.index + delta;
  if (index < 0) return;
  if (index >= detail.ids.length) {
    if (delta > 0 && libraryCanLoadMore()) {
      await loadLibrary(false);
      detail.ids = state.lib.items.map((item) => item.id);
      if (index >= detail.ids.length) {
        toast('No more videos in this view.');
        return;
      }
    } else {
      toast('No more videos in this view.');
      return;
    }
  }
  const ids = detail.ids.slice();
  try {
    await detail.persist();
  } catch (err) {
    /* navigating with unsaved edits: keep the last successful state */
  }
  detail.close();
  openDetail(ids[index], { ids, index });
}

/* ------------------------------- rate ----------------------------------- */

async function viewRate() {
  stopRatePlayer();
  $('#view').innerHTML = `
    <header class="page-head">
      <div>
        <h1>Rate</h1>
        <p class="muted" id="rate-sub">Loading queue...</p>
      </div>
      <div class="head-actions">
        <select class="select" id="rate-folder" title="Restrict the queue to a folder">${folderOptionsHtml(state.rate.folder)}</select>
        <button class="btn" id="rate-reload">Reload queue</button>
      </div>
    </header>
    <div id="rate-body"><div class="empty">Loading...</div></div>`;
  $('#rate-folder').onchange = (e) => {
    state.rate.folder = e.target.value;
    loadRateQueue();
  };
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
  body.innerHTML = '<div class="empty">Loading queue...</div>';
  try {
    const items = await invoke('rating_queue', { limit: 300, folder: state.rate.folder || null });
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
        <div class="file-path muted small" title="${esc(v.path)}">${esc(v.path)}</div>
        <dl class="meta-table">
          <dt>Duration</dt><dd>${v.durationMs ? fmtDur(v.durationMs) : '-'}</dd>
          <dt>Size</dt><dd>${fmtSize(v.size)}</dd>
          <dt>Resolution</dt><dd>${v.width ? `${v.width}\u00d7${v.height}` : '-'}</dd>
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
          <button class="btn tiny danger" id="r-delete">Move to trash</button>
        </div>
        <div class="kbd-hint">
          <kbd>Enter</kbd> save &amp; next \u00b7 <kbd>S</kbd> skip \u00b7 <kbd>1</kbd>-<kbd>5</kbd> rating<br>
          <kbd>N</kbd> NSFW \u00b7 <kbd>P</kbd> popping \u00b7 <kbd>D</kbd> destruction \u00b7 <kbd>F</kbd> deflation \u00b7 <kbd>I</kbd> inflation
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
  $('#r-delete').onclick = async () => {
    const ok = await confirmDialog({
      title: 'Move this video to trash?',
      message: `${v.fileName}\n\nIt will be moved to ${state.boot?.trashDir || 'the _trash folder'} and can be restored from the Trash view.`,
      okLabel: 'Move to trash',
      danger: true,
    });
    if (!ok) return;
    try {
      await invoke('trash_video', { id: v.id });
      state.rate.queue.splice(state.rate.index, 1);
      toast('Moved to trash.');
      await refreshMeta();
      renderRateCurrent();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  const sub = $('#rate-sub');
  if (sub) {
    const scope = state.rate.folder ? ` in ${folderLabel(state.rate.folder)}` : '';
    sub.textContent = `${state.stats.unreviewed ?? r.queue.length} unreviewed${scope} \u00b7 ${r.queue.length - r.index} left in this batch`;
  }
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
        <p class="muted" id="dup-sub">Looking for identical files...</p>
      </div>
      <div class="head-actions">
        <button class="btn" id="dup-refresh">Refresh</button>
      </div>
    </header>
    <div class="tabs">
      <button class="tab ${state.dupsTab === 'exact' ? 'active' : ''}" data-tab="exact">Exact (MD5)</button>
      <button class="tab ${state.dupsTab === 'similar' ? 'active' : ''}" data-tab="similar">Similar (Video DNA)</button>
    </div>
    <div id="dup-body"><div class="empty">Loading...</div></div>`;
  $('#dup-refresh').onclick = () => renderDupsTab();
  $$('.tab').forEach((tab) => {
    tab.onclick = () => {
      state.dupsTab = tab.dataset.tab;
      $$('.tab').forEach((t) => t.classList.toggle('active', t === tab));
      renderDupsTab();
    };
  });
  await renderDupsTab();
}

async function renderDupsTab() {
  const body = $('#dup-body');
  if (!body) return;
  if (state.dupsTab === 'exact') {
    body.innerHTML = '<div class="empty">Comparing MD5 hashes...</div>';
    try {
      state.dups = await invoke('duplicates');
    } catch (err) {
      toast(`Could not check duplicates: ${err}`, 'error');
      state.dups = [];
    }
    if (state.dupsTab !== 'exact') return;
    renderDups();
  } else {
    await renderSimilarTab();
  }
}

function renderDups() {
  const body = $('#dup-body');
  const sub = $('#dup-sub');
  if (!body) return;
  const groups = state.dups;
  const wasted = groups.reduce((sum, g) => sum + g.wasted, 0);
  if (sub) {
    sub.textContent = groups.length
      ? `${groups.length} duplicate group${groups.length === 1 ? '' : 's'} \u00b7 ${fmtSize(wasted)} recoverable`
      : 'No byte-identical duplicates found.';
  }
  if (!groups.length) {
    const pending = state.stats.hashPending || 0;
    const skipped = state.stats.hashSkipped || 0;
    body.innerHTML = `
      <div class="empty">
        <h2>No duplicates</h2>
        <p>Every hashed video has a unique MD5 fingerprint.</p>
        ${pending ? `<p class="warn">${pending} file${pending === 1 ? '' : 's'} still need hashing - run a scan to finish.</p>` : ''}
        ${skipped ? `<p class="muted small">${skipped} file${skipped === 1 ? '' : 's'} skipped: their size is unique, so they cannot be byte-identical to anything else.</p>` : ''}
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
    $$('[data-open-id]', groupEl).forEach((b) => {
      b.onclick = (e) => {
        e.stopPropagation();
        const ids = group.videos.map((item) => item.id);
        const openId = Number(b.dataset.openId);
        openDetail(openId, { ids, index: ids.indexOf(openId) });
      };
    });
    const auto = $('[data-auto-pick]', groupEl);
    if (auto) auto.onclick = () => { state.keep[md5] = autoPick(group.videos); syncKept(); };
    const trash = $('[data-trash]', groupEl);
    if (trash) trash.onclick = () => trashOthers(group, md5);
  });
}

/* ------------------------- similar (video DNA) --------------------------- */

async function renderSimilarTab() {
  const body = $('#dup-body');
  const sub = $('#dup-sub');
  if (!body) return;
  const stats = state.stats || {};
  const status = await invoke('dna_status').catch(() => null);
  if (state.dupsTab !== 'similar') return;
  const pending = stats.dnaPending ?? 0;
  const analyzed = stats.dnaDone ?? 0;

  let panel = '';
  if (status?.running) {
    const pct = status.total ? Math.round((100 * status.done) / status.total) : 0;
    panel = `
      <div class="scan-box">
        <div class="row between">
          <strong>Analyzing video DNA...</strong>
          <span class="muted small" id="dna-counter">${status.done} / ${status.total}</span>
        </div>
        <div class="progress"><div style="width:${Math.max(2, pct)}%"></div></div>
        <div class="muted small ellipsis" id="dna-current">${esc(status.current || '')}</div>
        <div class="row gap" style="margin-top:8px">
          <button class="btn tiny" id="dna-cancel">Stop</button>
        </div>
      </div>`;
  } else {
    panel = `
      <div class="scan-box">
        <div class="row between">
          <strong>${pending} video${pending === 1 ? '' : 's'} not analyzed</strong>
          <span class="muted small">${analyzed} analyzed</span>
        </div>
        <p class="muted small" style="margin:6px 0 8px">
          Video DNA compares frames sampled across each video, so it finds re-encodes,
          resolution changes and different bitrates that MD5 cannot.
        </p>
        <button class="btn primary" id="dna-start" ${pending ? '' : 'disabled'}>
          ${pending ? `Analyze ${pending} video${pending === 1 ? '' : 's'}` : 'Everything analyzed'}
        </button>
      </div>`;
  }

  if (sub) sub.textContent = 'Near-duplicates found by comparing sampled frames.';
  body.innerHTML = `${panel}<div id="similar-list"><div class="empty">${
    status?.running ? 'Analysis in progress...' : pending ? 'Analyze videos to see near-duplicates.' : 'Looking for near-duplicates...'
  }</div></div>`;

  if (status?.running) {
    $('#dna-cancel').onclick = async () => {
      try {
        await invoke('cancel_dna');
        toast('Stopping DNA analysis...');
      } catch (err) {
        toast(String(err), 'error');
      }
    };
    startDnaPolling();
    return;
  }

  $('#dna-start').onclick = async () => {
    try {
      await invoke('start_dna');
      await refreshMeta();
      renderSimilarTab();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  try {
    state.similar = await invoke('similar_duplicates');
  } catch (err) {
    state.similar = [];
    toast(`Could not compare videos: ${err}`, 'error');
  }
  if (state.dupsTab !== 'similar') return;
  renderSimilar();
}

function startDnaPolling() {
  if (state.dnaTimer) clearInterval(state.dnaTimer);
  state.dnaTimer = setInterval(async () => {
    if (state.view !== 'duplicates' || state.dupsTab !== 'similar') {
      clearInterval(state.dnaTimer);
      state.dnaTimer = null;
      return;
    }
    const status = await invoke('dna_status').catch(() => null);
    if (!status) return;
    if (!status.running) {
      clearInterval(state.dnaTimer);
      state.dnaTimer = null;
      await refreshMeta();
      renderSimilarTab();
      toast('Video DNA analysis finished.');
      return;
    }
    const pct = status.total ? Math.round((100 * status.done) / status.total) : 0;
    const bar = document.querySelector('#dup-body .progress > div');
    if (bar) bar.style.width = `${Math.max(2, pct)}%`;
    const counter = document.querySelector('#dna-counter');
    if (counter) counter.textContent = `${status.done} / ${status.total}`;
    const current = document.querySelector('#dna-current');
    if (current) current.textContent = status.current || '';
  }, 800);
}

function renderSimilar() {
  const list = $('#similar-list');
  if (!list) return;
  const groups = state.similar;
  if (!groups.length) {
    list.innerHTML = `<div class="empty"><h2>No similar videos</h2>
      <p>No near-duplicates found among the analyzed videos.</p></div>`;
    return;
  }
  const sub = $('#dup-sub');
  if (sub) sub.textContent = `${groups.length} similar group${groups.length === 1 ? '' : 's'} \u00b7 ${state.stats.dnaDone || 0} videos analyzed`;
  list.innerHTML = `<div class="dup-list">${groups.map(similarGroupHtml).join('')}</div>`;

  $$('.dup-group', list).forEach((groupEl) => {
    const index = Number(groupEl.dataset.similarIndex);
    const group = groups[index];
    if (!group) return;
    const key = `sim-${index}`;
    if (!state.keep[key]) state.keep[key] = autoPick(group.videos);
    const syncKept = () => {
      const keepId = state.keep[key];
      $$('tbody tr', groupEl).forEach((tr) => {
        tr.classList.toggle('kept', Number(tr.dataset.id) === keepId);
        const radio = $('input[type="radio"]', tr);
        if (radio) radio.checked = Number(tr.dataset.id) === keepId;
      });
    };
    syncKept();
    $$('input[type="radio"]', groupEl).forEach((radio) => {
      radio.onchange = () => {
        state.keep[key] = Number(radio.value);
        syncKept();
      };
    });
    $$('[data-reveal-id]', groupEl).forEach((b) => {
      b.onclick = () => invoke('reveal', { id: Number(b.dataset.revealId) })
        .catch((err) => toast(String(err), 'error'));
    });
    $$('[data-open-id]', groupEl).forEach((b) => {
      b.onclick = (e) => {
        e.stopPropagation();
        const ids = group.videos.map((item) => item.id);
        const openId = Number(b.dataset.openId);
        openDetail(openId, { ids, index: ids.indexOf(openId) });
      };
    });
    const auto = $('[data-auto-pick]', groupEl);
    if (auto) auto.onclick = () => { state.keep[key] = autoPick(group.videos); syncKept(); };
    const trash = $('[data-trash]', groupEl);
    if (trash) trash.onclick = () => trashOthers(group, key);
  });
}

function similarGroupHtml(group, index) {
  const rows = group.videos.map((v, rowIndex) => `
    <tr data-id="${v.id}">
      <td><input type="radio" name="keep-sim-${index}" value="${v.id}" ${state.keep[`sim-${index}`] === v.id ? 'checked' : ''}></td>
      <td>
        <button type="button" class="linkish fname" data-open-id="${v.id}" title="Open ${esc(v.fileName)}">${esc(v.fileName)}</button>
        ${rowIndex === 0 ? '<span class="badge best" title="Highest resolution / largest file">best</span>' : ''}
        <div class="muted small fname" title="${esc(v.path)}">${esc(v.path)}</div>
      </td>
      <td>${fmtSize(v.size)}</td>
      <td>${v.width ? `${v.width}\u00d7${v.height}` : '-'}</td>
      <td>${v.durationMs ? fmtDur(v.durationMs) : '-'}</td>
      <td>${v.viewCount}</td>
      <td>${fmtDate(v.addedAt)}</td>
      <td><button class="btn tiny" data-reveal-id="${v.id}">Folder</button></td>
    </tr>`).join('');
  return `
    <section class="dup-group" data-similar-index="${index}">
      <div class="dup-head">
        <span class="score">${Math.round(group.score * 100)}% frame match</span>
        <span>${group.videos.length} videos</span>
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
        <tbody>${rows}</tbody>
      </table>
      <div class="dup-actions">
        <button class="btn tiny" data-auto-pick>Auto-pick keeper</button>
        <button class="btn danger" data-trash>Move others to trash</button>
      </div>
    </section>`;
}

function autoPick(videos) {
  const sorted = videos.slice().sort((a, b) => {
    // Prefer the best quality copy: resolution, then bitrate (file size),
    // then duration. Identical copies fall through to views/age.
    const areaA = (a.width || 0) * (a.height || 0);
    const areaB = (b.width || 0) * (b.height || 0);
    if (areaB !== areaA) return areaB - areaA;
    if (b.size !== a.size) return b.size - a.size;
    if ((b.durationMs || 0) !== (a.durationMs || 0)) return (b.durationMs || 0) - (a.durationMs || 0);
    if (b.viewCount !== a.viewCount) return b.viewCount - a.viewCount;
    return a.addedAt - b.addedAt;
  });
  return sorted[0].id;
}

function dupGroupHtml(group) {
  return `
    <section class="dup-group" data-md5="${esc(group.md5)}">
      <div class="dup-head">
        <span class="hash">${esc(group.md5.slice(0, 16))}...</span>
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
                <button type="button" class="linkish fname" data-open-id="${v.id}" title="Open ${esc(v.fileName)}">${esc(v.fileName)}</button>
                <div class="muted small fname" title="${esc(v.path)}">${esc(v.path)}</div>
              </td>
              <td>${fmtSize(v.size)}</td>
              <td>${v.width ? `${v.width}\u00d7${v.height}` : '-'}</td>
              <td>${v.durationMs ? fmtDur(v.durationMs) : '-'}</td>
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

async function trashOthers(group, key = group.md5) {
  const keepId = state.keep[key];
  const others = group.videos.filter((v) => v.id !== keepId);
  if (!others.length) return;
  const trash = state.boot?.trashDir || 'the library _trash folder';
  const ok = await confirmDialog({
    title: 'Move duplicates to trash?',
    message: `Keeping:\n${group.videos.find((v) => v.id === keepId)?.fileName}\n\n` +
      `Moving ${others.length} file${others.length === 1 ? '' : 's'} to:\n${trash}\n\n` +
      'They can be restored from the Trash view.',
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
  if (String(key).startsWith('sim-')) {
    state.similar = state.similar.filter((g) => g !== group);
  } else {
    state.dups = state.dups.filter((g) => g.md5 !== group.md5);
  }
  delete state.keep[key];
  toast(`Moved ${moved} file${moved === 1 ? '' : 's'} to trash.`);
  if (state.dupsTab === 'similar') renderSimilarTab();
  else renderDups();
  refreshMeta();
}

/* -------------------------------- trash --------------------------------- */

async function viewTrash() {
  $('#view').innerHTML = `
    <header class="page-head">
      <div>
        <h1>Trash</h1>
        <p class="muted" id="trash-sub">Loading...</p>
      </div>
      <div class="head-actions">
        <button class="btn" id="trash-refresh">Refresh</button>
      </div>
    </header>
    <div id="trash-body"><div class="empty">Loading...</div></div>`;
  $('#trash-refresh').onclick = () => viewTrash();
  state.trashSelected = new Set();
  try {
    state.trash = await invoke('trash_list');
  } catch (err) {
    toast(`Could not read the trash: ${err}`, 'error');
    state.trash = [];
  }
  renderTrash();
}

function renderTrash() {
  const body = $('#trash-body');
  const sub = $('#trash-sub');
  if (!body) return;
  const entries = state.trash;
  const selected = state.trashSelected;
  const total = entries.reduce((sum, entry) => sum + entry.size, 0);
  if (sub) {
    sub.textContent = entries.length
      ? `${entries.length} file${entries.length === 1 ? '' : 's'} \u00b7 ${fmtSize(total)}`
      : 'Empty.';
  }
  if (!entries.length) {
    body.innerHTML = `<div class="empty">
        <h2>Trash is empty</h2>
        <p>Files moved out of the library land here and can be restored.</p>
      </div>`;
    return;
  }

  body.innerHTML = `
    <div class="bulk-bar" style="position:static;margin-bottom:12px">
      <span class="small" id="trash-sel-count">${selected.size} selected</span>
      <button class="btn tiny" id="trash-select-all">Select all</button>
      <button class="btn tiny" id="trash-restore-sel" ${selected.size ? '' : 'disabled'}>Restore selected</button>
      <button class="btn tiny danger" id="trash-del-sel" ${selected.size ? '' : 'disabled'}>Delete selected</button>
      <button class="btn tiny danger" id="trash-del-all">Delete all</button>
    </div>
    <table class="dup-table trash-table">
      <thead>
        <tr>
          <th style="width:30px"></th>
          <th>File</th>
          <th style="width:90px">Size</th>
          <th>Original location</th>
          <th style="width:110px">Removed</th>
          <th style="width:190px"></th>
        </tr>
      </thead>
      <tbody>
        ${entries.map((entry) => `
          <tr data-id="${entry.id}">
            <td><input type="checkbox" class="trash-check" data-id="${entry.id}" ${selected.has(entry.id) ? 'checked' : ''}></td>
            <td><div class="fname" title="${esc(entry.trashedPath)}">${esc(entry.fileName)}</div></td>
            <td>${fmtSize(entry.size)}</td>
            <td><div class="muted small fname" title="${esc(entry.originalPath)}">${esc(entry.originalPath)}</div></td>
            <td>${fmtDate(entry.trashedAt)}</td>
            <td>
              <button class="btn tiny" data-restore>Restore</button>
              <button class="btn tiny danger" data-purge>Delete</button>
            </td>
          </tr>`).join('')}
      </tbody>
    </table>`;

  const refreshBar = () => {
    const label = $('#trash-sel-count');
    if (label) label.textContent = `${selected.size} selected`;
    const restore = $('#trash-restore-sel');
    if (restore) restore.disabled = selected.size === 0;
    const del = $('#trash-del-sel');
    if (del) del.disabled = selected.size === 0;
  };

  const syncChecks = () => {
    $$('.trash-check').forEach((box) => {
      box.checked = selected.has(Number(box.dataset.id));
    });
    refreshBar();
  };

  $$('.trash-check').forEach((box) => {
    box.onchange = () => {
      const id = Number(box.dataset.id);
      if (box.checked) selected.add(id);
      else selected.delete(id);
      refreshBar();
    };
  });

  const bulkRestore = async (ids) => {
    let ok = 0;
    let failed = 0;
    for (const id of ids) {
      try {
        await invoke('restore_trash', { id });
        ok += 1;
      } catch (err) {
        failed += 1;
      }
    }
    toast(`Restored ${ok} file${ok === 1 ? '' : 's'}${failed ? `, ${failed} failed` : ''}.`, failed ? 'error' : 'info');
    selected.clear();
    await refreshMeta();
    viewTrash();
  };

  const bulkDelete = async (ids) => {
    let ok = 0;
    let failed = 0;
    for (const id of ids) {
      try {
        await invoke('delete_trash', { id });
        ok += 1;
      } catch (err) {
        failed += 1;
      }
    }
    toast(`Deleted ${ok} file${ok === 1 ? '' : 's'}${failed ? `, ${failed} failed` : ''}.`, failed ? 'error' : 'info');
    selected.clear();
    await refreshMeta();
    viewTrash();
  };

  $('#trash-select-all').onclick = () => {
    entries.forEach((entry) => selected.add(entry.id));
    syncChecks();
  };

  $('#trash-restore-sel').onclick = () => {
    const ids = Array.from(selected);
    if (ids.length) bulkRestore(ids);
  };

  $('#trash-del-sel').onclick = async () => {
    const ids = Array.from(selected);
    if (!ids.length) return;
    const ok = await confirmDialog({
      title: `Delete ${ids.length} file${ids.length === 1 ? '' : 's'} permanently?`,
      message: 'They will be erased from disk. This cannot be undone.',
      okLabel: 'Delete forever',
      danger: true,
    });
    if (ok) bulkDelete(ids);
  };

  $('#trash-del-all').onclick = async () => {
    if (!entries.length) return;
    const ok = await confirmDialog({
      title: `Delete all ${entries.length} files permanently?`,
      message: 'Every file in the trash will be erased from disk. This cannot be undone.',
      okLabel: 'Delete everything',
      danger: true,
    });
    if (ok) bulkDelete(entries.map((entry) => entry.id));
  };

  $$('tbody tr', body).forEach((row) => {
    const id = Number(row.dataset.id);
    const restore = $('[data-restore]', row);
    if (restore) {
      restore.onclick = async () => {
        try {
          const path = await invoke('restore_trash', { id });
          toast(`Restored to ${path}`);
          await refreshMeta();
          viewTrash();
        } catch (err) {
          toast(String(err), 'error');
        }
      };
    }
    const purge = $('[data-purge]', row);
    if (purge) {
      purge.onclick = async () => {
        const ok = await confirmDialog({
          title: 'Delete permanently?',
          message: 'This file will be erased from disk. It cannot be restored.',
          okLabel: 'Delete forever',
          danger: true,
        });
        if (!ok) return;
        try {
          await invoke('delete_trash', { id });
          toast('File deleted.');
          await refreshMeta();
          viewTrash();
        } catch (err) {
          toast(String(err), 'error');
        }
      };
    }
  });
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
        <div class="muted small" style="margin-top:4px">${scan.added} new \u00b7 ${scan.updated} changed \u00b7 ${scan.failed} failed</div>
      </div>`;
  }
  const when = scan.finishedAt ? new Date(scan.finishedAt * 1000).toLocaleString() : '';
  const error = scan.error ? `<p class="warn small">${esc(scan.error)}</p>` : '';
  return `
    <div class="scan-box">
      <div class="row between"><strong>Last scan</strong><span class="muted small">${esc(when)}</span></div>
      <div class="muted small" style="margin-top:6px">
        ${scan.added} new \u00b7 ${scan.updated} changed ... ${scan.removed} missing \u00b7 ${scan.failed} failed
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
          <button class="btn" id="set-pick">Choose...</button>
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
          <button class="btn" id="set-db-dir">Change folder...</button>
          ${b.dataDir && b.defaultDataDir && b.dataDir !== b.defaultDataDir
            ? '<button class="btn" id="set-db-default">Use default</button>' : ''}
        </div>
        ${b.dataWarning ? `<p class="warn small">${esc(b.dataWarning)}</p>` : ''}
        <p class="muted small">Moving copies <code>library.db</code> to the chosen folder; the current file stays behind as a backup. An existing <code>library.db</code> in the target folder is renamed aside first.</p>
        <div class="kv" style="margin-top:12px">Cache: <code>${esc(b.cacheDir || '')}</code> - <span id="cache-size">...</span></div>
        <div class="kv">Trash: <code>${esc(b.trashDir || '-')}</code></div>
        <div class="row gap" style="margin-top:10px">
          <button class="btn" id="set-clear-cache">Clear cache</button>
          <button class="btn" id="set-refresh-cache">Recalculate size</button>
        </div>
        <p class="muted small">The cache holds generated preview frames and converted MP4s; it is rebuilt on demand.</p>
      </section>

      <section class="panel">
        <h2>Indexing &amp; cache</h2>
        <div class="field">
          <label>Duplicate hashing</label>
          <select class="select" id="set-hash-mode">
            <option value="duplicates" ${b.hashMode === 'duplicates' ? 'selected' : ''}>Only files with a shared size (fast)</option>
            <option value="all" ${b.hashMode === 'all' ? 'selected' : ''}>Hash every file (slow)</option>
          </select>
          <p class="muted small">A file whose size is unique cannot be byte-identical to anything else, so it is skipped. Change the mode, then run a scan.</p>
        </div>
        <div class="field">
          <label>Cache limit (MB)</label>
          <div class="path-row">
            <input class="input" id="set-cache-limit" type="number" min="0" step="256" value="${b.cacheLimitMb ?? 4096}">
            <button class="btn" id="set-cache-apply">Apply</button>
          </div>
          <p class="muted small">0 = unlimited. The oldest previews/conversions are removed when the cache grows past this.</p>
        </div>
        <button class="btn" id="set-clean-cache">Clean up now</button>
      </section>

      <section class="panel">
        <h2>Tags</h2>
        <div class="tag-manager">
          ${state.tags.length
            ? state.tags.map((t) => `
              <div class="tag-row" data-tag="${esc(t.name)}">
                <span class="tag-name ellipsis">${esc(t.name)}</span>
                <span class="muted small">${t.count}</span>
                <button class="btn tiny" data-tag-rename>Rename</button>
                <button class="btn tiny danger" data-tag-delete>Delete</button>
              </div>`).join('')
            : '<p class="muted small">No tags yet.</p>'}
        </div>
        <p class="muted small">Renaming a tag to a name that already exists merges the two.</p>
      </section>

      <section class="panel">
        <h2>Toys</h2>
        <div class="tag-manager">
          ${state.toys.length
            ? state.toys.map((t) => `
              <div class="toy-row" data-toy="${esc(t.name)}">
                <span class="tag-name ellipsis">${esc(t.name)}</span>
                <span class="muted small">${t.count}</span>
                <button class="btn tiny" data-toy-rename>Rename</button>
                <button class="btn tiny danger" data-toy-delete>Delete</button>
              </div>`).join('')
            : '<p class="muted small">No toys yet.</p>'}
        </div>
        <p class="muted small">Toys are a separate field from tags, so you can filter by the inflatable (dolphin, orca, pool...) on its own. Renaming into an existing name merges them.</p>
      </section>

      <section class="panel">
        <h2>Data</h2>
        <div class="row gap" style="flex-wrap:wrap">
          <button class="btn" id="set-backup">Back up database...</button>
          <button class="btn" id="set-export">Export metadata...</button>
          <button class="btn" id="set-import">Import metadata...</button>
        </div>
        <p class="muted small">Export writes tags, ratings and flags for every video as JSON; import matches entries back by MD5 hash, then by path.</p>
      </section>

      <section class="panel">
        <h2>Library stats</h2>
        <div class="kv">Videos: ${state.stats.total || 0} (${fmtSize(state.stats.totalSize || 0)})</div>
        <div class="kv">Reviewed: ${state.stats.reviewed || 0} \u00b7 Unreviewed: ${state.stats.unreviewed || 0}</div>
        <div class="kv">Tagged: ${state.stats.tagged || 0} \u00b7 NSFW: ${state.stats.nsfw || 0}</div>
        <div class="kv">Duplicate groups: ${state.stats.duplicates || 0}</div>
        <div class="kv">Hash: ${state.stats.hashPending || 0} pending \u00b7 ${state.stats.hashSkipped || 0} skipped</div>
        <div class="kv">Video DNA: ${state.stats.dnaDone || 0} analyzed \u00b7 ${state.stats.dnaPending || 0} to go</div>
        <div class="kv">Trash: ${state.stats.trash || 0} file${state.stats.trash === 1 ? '' : 's'}</div>
        <div class="kv">Total duration: ${fmtDur(state.stats.totalDurationMs || 0)}</div>
      </section>

      <section class="panel">
        <h2>Shortcuts (Rate view)</h2>
        <ul class="shortcut-list">
          <li><span><kbd>Enter</kbd></span><span>Save &amp; next</span></li>
          <li><span><kbd>S</kbd></span><span>Skip (keeps unreviewed)</span></li>
          <li><span><kbd>1</kbd>-<kbd>5</kbd></span><span>Set star rating</span></li>
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
      toast('Stopping scan...');
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

  $('#set-hash-mode').onchange = async (e) => {
    try {
      await invoke('set_hash_mode', { mode: e.target.value });
      state.boot.hashMode = e.target.value;
      toast('Hash mode saved. Run a scan to apply it.');
      await refreshMeta();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $('#set-cache-apply').onclick = async () => {
    const mb = Math.max(0, Number($('#set-cache-limit').value) || 0);
    try {
      const freed = await invoke('set_cache_limit', { mb });
      state.boot.cacheLimitMb = mb;
      toast(freed ? `Cache limit saved, freed ${fmtSize(freed)}.` : 'Cache limit saved.');
      loadCacheSize();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $('#set-clean-cache').onclick = async () => {
    try {
      const freed = await invoke('clean_cache');
      toast(freed ? `Freed ${fmtSize(freed)}.` : 'Cache is under the limit.');
      loadCacheSize();
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $$('.tag-row').forEach((row) => {
    const name = row.dataset.tag;
    const rename = $('[data-tag-rename]', row);
    if (rename) {
      rename.onclick = async () => {
        const next = await promptDialog('Rename tag', 'New name (existing name merges)', name);
        if (!next || next === name) return;
        try {
          await invoke('rename_tag', { old: name, new: next });
          toast('Tag renamed.');
          await refreshMeta();
          viewSettings();
        } catch (err) {
          toast(String(err), 'error');
        }
      };
    }
    const remove = $('[data-tag-delete]', row);
    if (remove) {
      remove.onclick = async () => {
        const ok = await confirmDialog({
          title: `Delete tag "${name}"?`,
          message: 'The tag is removed from every video.',
          okLabel: 'Delete tag',
          danger: true,
        });
        if (!ok) return;
        try {
          await invoke('delete_tag', { name });
          toast('Tag deleted.');
          await refreshMeta();
          viewSettings();
        } catch (err) {
          toast(String(err), 'error');
        }
      };
    }
  });

  $$('.toy-row').forEach((row) => {
    const name = row.dataset.toy;
    const rename = $('[data-toy-rename]', row);
    if (rename) {
      rename.onclick = async () => {
        const next = await promptDialog('Rename toy', 'New name (existing name merges)', name);
        if (!next || next === name) return;
        try {
          await invoke('rename_toy', { old: name, new: next });
          toast('Toy renamed.');
          await refreshMeta();
          viewSettings();
        } catch (err) {
          toast(String(err), 'error');
        }
      };
    }
    const remove = $('[data-toy-delete]', row);
    if (remove) {
      remove.onclick = async () => {
        const ok = await confirmDialog({
          title: `Delete toy "${name}"?`,
          message: 'The toy is removed from every video.',
          okLabel: 'Delete toy',
          danger: true,
        });
        if (!ok) return;
        try {
          await invoke('delete_toy', { name });
          toast('Toy deleted.');
          await refreshMeta();
          viewSettings();
        } catch (err) {
          toast(String(err), 'error');
        }
      };
    }
  });

  $('#set-backup').onclick = async () => {
    try {
      const path = await invoke('backup_database');
      if (path) toast(`Backup written to ${path}`);
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $('#set-export').onclick = async () => {
    try {
      const path = await invoke('export_library');
      if (path) toast(`Metadata exported to ${path}`);
    } catch (err) {
      toast(String(err), 'error');
    }
  };

  $('#set-import').onclick = async () => {
    try {
      const res = await invoke('import_library');
      if (!res) return;
      toast(
        `Imported ${res.matched} of ${res.total} entries${res.unmatched ? ` (${res.unmatched} not found)` : ''}.`,
        res.unmatched ? 'error' : 'info',
      );
      await refreshMeta();
      viewSettings();
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
      else toast(`Scan finished - ${next.added} new, ${next.updated} changed, ${next.removed} missing.`);
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
      line.textContent = 'Scan error - see Settings';
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
  if (state.detail && (e.key === 'ArrowRight' || e.key === 'ArrowLeft') && $('#d-video')) {
    const tag = (e.target?.tagName || '').toLowerCase();
    const typing = tag === 'input' || tag === 'textarea' || tag === 'select';
    if (!document.fullscreenElement && typing) return;
    e.preventDefault();
    navigateDetail(e.key === 'ArrowRight' ? 1 : -1);
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



