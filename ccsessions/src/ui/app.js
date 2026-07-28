// ccsessions の画面 — 設定とキャラクタービルダー。
//
// # 役割
// **顔は 1 ピクセルも描かない。** 輪郭も目も線も、サーバが `face::svg` で
// 描いた SVG をそのまま貼るだけ（`ui_cmd.rs` の doc 参照）。ここが持つのは
// 「いま何が選ばれているか」＝ `config` と、その見せ方だけ。
//
// # UI の状態 = config
// 画面のすべてのつまみは `config` の 1 フィールドに対応し、`config` はそのまま
// 保存・読み込みされる JSON。だから「見えているもの」と「保存されるもの」が
// ずれない。
//
// # 設定のキーも、パーツのカテゴリもここには書かない
// 設定は `/api/config` が返すスキーマ、パーツは `/api/parts` が返す表を列挙して
// 描く。設定を 1 つ足すときに触るのは core のスキーマだけ、パーツを足すときは
// `parts.rs` の表だけ、という設計を画面側でも守る。

'use strict';

const $ = (sel) => document.querySelector(sel);

const state = {
  meta: null,      // /api/parts の中身
  config: null,    // 保存・読み込みの単位そのもの（キャラクター）
  tab: 'face',     // 開いているカテゴリ
  thumbs: {},      // カテゴリ id → [{id, svg}]
  mainSvg: '',     // いま出ているプレビューの SVG（書き出しに使う）
  seq: 0,          // 応答の追い越し対策
  settings: null,  // /api/config の中身（スキーマ + 現在値 + 顔の選択肢）
  view: 'settings',
  builderReady: false, // キャラクター画面を初期化したか（開くまで作らない）
};

// カテゴリの種類ごとに出すつまみ。[key, ラベル, 最小, 最大, 刻み, 既定]
const TWEAKS = {
  face: [['scale', '大きさ', 0.6, 1.6, 0.02, 1]],
  eyes: [
    ['scale', '大きさ', 0.5, 1.6, 0.02, 1],
    ['dy', '縦位置', -0.15, 0.15, 0.005, 0],
    ['gap', '間隔', 0.4, 2.0, 0.02, 1],
  ],
  line: [
    ['scale', '幅', 0.3, 1.6, 0.02, 1],
    ['dx', '横位置', -0.25, 0.25, 0.01, 0],
    ['dy', '縦位置', -0.15, 0.15, 0.005, 0],
  ],
};

// ---------------------------------------------------------------------------
// 起動
// ---------------------------------------------------------------------------

boot();

async function boot() {
  bindViews();
  // **出し分けは JS が 1 回やる。** HTML の `hidden` 属性だけに任せると、
  // ヘッダの「キャラクター専用」の部品を消す口がここに無くなる。
  showView(state.view);
  // 立ち上がりは設定画面。キャラクター側（パーツ表とプレビュー）は
  // 開いたときに初期化する — 設定を 1 つ変えたいだけの人に、30 種 × 5 カテゴリの
  // サムネイル生成を待たせない。
  await loadSettings();
}

/** キャラクター画面を初めて開いたときの初期化。2 度目以降は何もしない。 */
async function bootBuilder() {
  if (state.builderReady) return;
  try {
    state.meta = await getJson('/api/parts');
  } catch (e) {
    toast('パーツ一覧を取れません: ' + e.message, true);
    return;
  }
  state.builderReady = true;
  state.config = clone(state.meta.default);

  buildTabs();
  buildEyeColors();
  bindHeader();
  bindKeys();
  syncIdent();
  refresh();
  loadSavedList();
}

// ---------------------------------------------------------------------------
// 画面の切り替え
// ---------------------------------------------------------------------------

function bindViews() {
  for (const b of $('#views').children) {
    b.onclick = () => showView(b.dataset.view);
  }
}

function showView(view) {
  state.view = view;
  for (const b of $('#views').children) b.classList.toggle('on', b.dataset.view === view);
  $('#view-settings').hidden = view !== 'settings';
  $('#view-builder').hidden = view !== 'builder';
  for (const e of document.querySelectorAll('.builder-only')) e.hidden = view !== 'builder';
  if (view === 'builder') bootBuilder();
  // 顔を保存してから設定へ戻ると選択肢が増えているので、毎回取り直す。
  if (view === 'settings' && state.settings) loadSettings();
}

// ---------------------------------------------------------------------------
// 設定
// ---------------------------------------------------------------------------

async function loadSettings() {
  try {
    state.settings = await getJson('/api/config');
  } catch (e) {
    $('#fields').textContent = '設定を読めません: ' + e.message;
    return;
  }
  $('#config-path').textContent = state.settings.path;
  renderFields();
  renderFacePicker();
}

/** 設定 1 項目を保存する。**画面の見た目はサーバが返した値に合わせ直す** */
async function setField(key, value) {
  let res;
  try {
    res = await postJson('/api/config', { key, value });
  } catch (e) {
    toast(e.message, true);
    // 拒否されたときに画面だけ変わったままにしない。
    loadSettings();
    return;
  }
  const f = state.settings.fields.find((f) => f.key === key);
  if (f) f.value = res.value;
  if (key === 'design') renderFacePicker();
  toast(res.message);
}

function renderFields() {
  const box = $('#fields');
  box.textContent = '';
  for (const f of state.settings.fields) {
    // 顔はプレビュー付きの専用ピッカー（#face-picker）で選ぶので、
    // ここには重複して出さない。
    if (f.kind === 'face') continue;
    box.append(fieldRow(f));
  }
}

function fieldRow(f) {
  const row = el('div');
  row.className = 'field';
  const head = el('div', '', 'field-head');
  head.append(el('span', f.label, 'field-label'), el('span', f.key, 'field-key'));
  row.append(head, fieldInput(f));
  if (f.help) row.append(el('p', f.help, 'hint'));
  return row;
}

function fieldInput(f) {
  if (f.kind === 'choice') return choiceInput(f);
  if (f.kind === 'bool') return boolInput(f);
  if (f.kind === 'int') return intInput(f);
  if (f.kind === 'coord') return coordInput(f);
  return el('span', f.value);
}

function choiceInput(f) {
  const wrap = el('div', '', 'seg');
  for (const c of f.choices) {
    const b = el('button', c.label);
    b.classList.toggle('on', c.id === f.value);
    b.onclick = () => {
      f.value = c.id;
      for (const other of wrap.children) other.classList.toggle('on', other === b);
      setField(f.key, c.id);
    };
    wrap.append(b);
  }
  return wrap;
}

function boolInput(f) {
  const wrap = el('label', '', 'toggle');
  const cb = el('input');
  cb.type = 'checkbox';
  cb.checked = f.value === 'true';
  const text = el('span', cb.checked ? 'on' : 'off', 'toggle-text');
  cb.onchange = () => {
    text.textContent = cb.checked ? 'on' : 'off';
    setField(f.key, cb.checked);
  };
  wrap.append(cb, text);
  return wrap;
}

function intInput(f) {
  const wrap = el('div', '', 'num');
  const input = el('input');
  input.type = 'number';
  input.min = f.min;
  input.max = f.max;
  input.value = f.value;
  // 打っている途中で毎文字保存すると「1」「12」…が全部検証に掛かる。
  // 離れたとき（と Enter）だけ送る。
  input.onchange = () => setField(f.key, input.value);
  wrap.append(input, el('span', f.unit, 'unit'));
  return wrap;
}

function coordInput(f) {
  const wrap = el('div', '', 'num');
  wrap.append(el('span', f.value === 'auto' ? '既定の位置' : f.value + ' pt', 'coord-value'));
  if (f.value !== 'auto') {
    const b = el('button', '既定に戻す');
    b.onclick = () => setField(f.key, 'auto').then(loadSettings);
    wrap.append(b);
  }
  return wrap;
}

/** `design` の選択肢。プレビューはサーバが `face::svg` で描いたもの。 */
function renderFacePicker() {
  const box = $('#face-picker');
  const current = state.settings.fields.find((f) => f.key === 'design');
  box.textContent = '';
  for (const face of state.settings.faces) {
    const chip = el('div');
    chip.className = 'chip' + (face.id === current.value ? ' on' : '');
    chip.title = face.id;
    const art = el('div');
    // サーバが描いた SVG（`face::svg` が id / label をエスケープ済み）。
    art.innerHTML = face.svg;
    chip.append(art, el('span', face.label, 'name'));
    if (!face.builtin) chip.append(el('span', '自作', 'badge'));
    chip.onclick = () => {
      current.value = face.id;
      renderFacePicker();
      setField('design', face.id);
    };
    box.append(chip);
  }
}

// ---------------------------------------------------------------------------
// 通信
// ---------------------------------------------------------------------------

async function getJson(url) {
  const r = await fetch(url);
  const v = await r.json();
  if (!r.ok) throw new Error(v.error || r.statusText);
  return v;
}

async function postJson(url, body) {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  const v = await r.json();
  if (!r.ok) throw new Error(v.error || r.statusText);
  return v;
}

let timer = null;
/** プレビューを取り直す。連打しても最後の 1 回だけ効く。 */
function refresh() {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const my = ++state.seq;
    let res;
    try {
      res = await postJson('/api/preview', state.config);
    } catch (e) {
      toast('プレビューを作れません: ' + e.message, true);
      return;
    }
    if (my !== state.seq) return; // 追い越された応答は捨てる
    state.thumbs = res.thumbs;
    state.mainSvg = res.main;
    renderPreview(res);
    renderGrid();
    renderProblems(res);
    $('#toml').textContent = res.toml;
  }, 110);
}

// ---------------------------------------------------------------------------
// カテゴリのタブとパーツの一覧
// ---------------------------------------------------------------------------

function buildTabs() {
  const nav = $('#tabs');
  nav.textContent = '';
  for (const cat of state.meta.categories) {
    const b = el('button', cat.label);
    b.dataset.cat = cat.id;
    b.onclick = () => {
      state.tab = cat.id;
      renderGrid();
    };
    nav.append(b);
  }
  renderGrid();
}

function category(id) {
  return state.meta.categories.find((c) => c.id === id);
}

function renderGrid() {
  for (const b of $('#tabs').children) b.classList.toggle('on', b.dataset.cat === state.tab);

  const cat = category(state.tab);
  const grid = $('#grid');
  grid.textContent = '';

  // 見出し + 左右送り（サムネイルを見ずに順に試したいとき用）。
  const head = el('div');
  head.className = 'picker-head';
  head.append(el('span', `${cat.label} — ${cat.variants.length} 種`));
  const nav = el('div');
  nav.className = 'nav';
  const prev = el('button', '←');
  const next = el('button', '→');
  prev.onclick = () => step(-1);
  next.onclick = () => step(1);
  prev.title = '前のパーツ（←）';
  next.title = '次のパーツ（→）';
  nav.append(prev, next);
  head.append(nav);
  grid.append(head);
  head.style.gridColumn = '1 / -1';

  const chosen = state.config.parts[cat.id];
  const thumbs = state.thumbs[cat.id] || [];
  for (const v of cat.variants) {
    const chip = el('div');
    chip.className = 'chip' + (v.id === chosen ? ' on' : '');
    chip.setAttribute('role', 'option');
    chip.setAttribute('aria-selected', String(v.id === chosen));
    chip.title = v.id;

    const t = thumbs.find((x) => x.id === v.id);
    const box = el('div');
    // サムネイルはサーバが描いた SVG（自前で生成した文字列で、`face::svg` が
    // id / label をエスケープ済み）。
    box.innerHTML = t ? t.svg : '';
    chip.append(box, el('span', v.label, 'name'));
    chip.onclick = () => setPart(cat.id, v.id);
    grid.append(chip);
  }

  renderTweaks(cat);
}

function step(dir) {
  const cat = category(state.tab);
  const ids = cat.variants.map((v) => v.id);
  const i = ids.indexOf(state.config.parts[cat.id]);
  const next = ids[(i + dir + ids.length) % ids.length];
  setPart(cat.id, next);
}

function setPart(catId, partId) {
  state.config.parts[catId] = partId;
  renderGrid(); // 選択枠だけ先に動かす（サムネイルは refresh で入れ替わる）
  refresh();
}

// ---------------------------------------------------------------------------
// 微調整
// ---------------------------------------------------------------------------

function tweakOf(catId) {
  if (!state.config.tweaks[catId]) {
    state.config.tweaks[catId] = { dx: 0, dy: 0, scale: 1, gap: 1, bar: null };
  }
  return state.config.tweaks[catId];
}

function renderTweaks(cat) {
  const box = $('#tweaks');
  box.textContent = '';
  const fields = TWEAKS[cat.kind] || TWEAKS.line;
  const t = tweakOf(cat.id);

  for (const [key, label, min, max, stepv, dflt] of fields) {
    const row = el('div');
    row.className = 'tweak';
    const val = el('span', fmt(t[key] ?? dflt), 'val');
    const input = el('input');
    input.type = 'range';
    input.min = min;
    input.max = max;
    input.step = stepv;
    input.value = t[key] ?? dflt;
    input.oninput = () => {
      t[key] = parseFloat(input.value);
      val.textContent = fmt(t[key]);
      refresh();
    };
    row.append(el('span', label), input, val);
    box.append(row);
  }

  // 線のカテゴリだけ「bar にも描く」を出す。
  if (cat.kind === 'line') {
    const wrap = el('label');
    wrap.className = 'tweak-bar';
    const cb = el('input');
    cb.type = 'checkbox';
    cb.checked = t.bar === null || t.bar === undefined ? cat.on_bar : t.bar;
    cb.onchange = () => {
      t.bar = cb.checked;
      refresh();
    };
    wrap.append(cb, el('span', 'メニューバー（bar）にも描く'));
    box.append(wrap);
    const note = el('p', 'bar は狭いので、線を増やすとシルエットと目が読みにくくなる。', 'hint');
    box.append(note);
  }

  const reset = el('button', 'このカテゴリの微調整を戻す');
  reset.className = 'reset';
  reset.onclick = () => {
    delete state.config.tweaks[cat.id];
    renderGrid();
    refresh();
  };
  box.append(reset);
}

// ---------------------------------------------------------------------------
// プレビュー
// ---------------------------------------------------------------------------

function renderPreview(res) {
  const box = $('#preview');
  box.innerHTML = res.main;
  box.classList.toggle('bar', state.config.preview.size === 'bar');

  for (const b of $('#sizes').children) {
    b.classList.toggle('on', b.dataset.size === state.config.preview.size);
    b.onclick = () => {
      state.config.preview.size = b.dataset.size;
      refresh();
    };
  }

  const row = $('#states');
  row.textContent = '';
  for (const s of res.states) {
    const cell = el('div');
    cell.className = 'state' + (s.id === state.config.preview.state ? ' on' : '');
    const box2 = el('div');
    box2.innerHTML = s.svg;
    cell.append(box2, el('span', s.label, 'name'));
    cell.onclick = () => {
      state.config.preview.state = s.id;
      refresh();
    };
    row.append(cell);
  }

  $('#fit').textContent =
    res.eye_fit < 0.999
      ? `目がこの輪郭に収まらないので ${Math.round(res.eye_fit * 100)}% に縮めた。`
      : '';
}

function renderProblems(res) {
  const box = $('#problems');
  box.textContent = '';
  for (const p of res.problems) {
    box.append(problemRow(p.code, p.message, ''));
  }
  if (res.warning) {
    box.append(problemRow(res.warning.code, res.warning.message, 'warn'));
  }
  if (res.problems.length === 0) {
    box.prepend(problemRow('OK', '検証を通っている。保存できる。', 'ok'));
  }
}

function problemRow(code, message, cls) {
  const d = el('div');
  d.className = 'problem' + (cls ? ' ' + cls : '');
  d.append(el('code', code), el('span', message));
  return d;
}

// ---------------------------------------------------------------------------
// 名前・id・目の色
// ---------------------------------------------------------------------------

function syncIdent() {
  $('#f-name').value = state.config.name;
  $('#f-id').value = state.config.id;
  $('#f-author').value = state.config.author || '';
  $('#f-eyecolor').value = state.config.eye_color;
}

function buildEyeColors() {
  const sel = $('#f-eyecolor');
  sel.textContent = '';
  for (const c of state.meta.eye_colors) {
    const o = el('option', c.label);
    o.value = c.id;
    sel.append(o);
  }
  sel.onchange = () => {
    state.config.eye_color = sel.value;
    refresh();
  };
}

function bindHeader() {
  $('#f-name').oninput = (e) => {
    state.config.name = e.target.value;
    refresh();
  };
  $('#f-id').oninput = (e) => {
    state.config.id = e.target.value;
    refresh();
  };
  $('#f-author').oninput = (e) => {
    state.config.author = e.target.value || null;
    refresh();
  };

  $('#b-random').onclick = randomize;
  $('#b-save').onclick = save;
  $('#b-json').onclick = () =>
    download(`${safeName()}.ccchar.json`, JSON.stringify(state.config, null, 2), 'application/json');
  $('#b-open').onclick = () => $('#f-file').click();
  $('#f-file').onchange = openFile;
  $('#b-svg').onclick = () => download(`${safeName()}.svg`, state.mainSvg, 'image/svg+xml');
  $('#b-png').onclick = downloadPng;
}

function bindKeys() {
  document.addEventListener('keydown', (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const tag = document.activeElement && document.activeElement.tagName;
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') return;
    if (e.key === 'ArrowLeft') { step(-1); e.preventDefault(); }
    else if (e.key === 'ArrowRight') { step(1); e.preventDefault(); }
    else if (e.key === 'r' || e.key === 'R') { randomize(); e.preventDefault(); }
  });
}

// ---------------------------------------------------------------------------
// ランダム・保存・読み込み
// ---------------------------------------------------------------------------

async function randomize() {
  // 種はこちらで作る。サーバは同じ種なら同じ顔を返すので、
  // 「さっきの顔」に戻したくなったら種を控えておけばよい。
  const seed = Math.floor(Math.random() * 2 ** 32);
  try {
    state.config = await postJson('/api/random?seed=' + seed, state.config);
  } catch (e) {
    toast('ランダム生成に失敗: ' + e.message, true);
    return;
  }
  syncIdent();
  renderGrid();
  refresh();
}

async function save() {
  try {
    const r = await postJson('/api/save', state.config);
    toast(r.message);
    loadSavedList();
  } catch (e) {
    toast(e.message, true);
  }
}

async function loadSavedList() {
  const box = $('#saved');
  let data;
  try {
    data = await getJson('/api/saved');
  } catch {
    box.textContent = '';
    return;
  }
  box.textContent = '';
  if (data.items.length === 0) {
    box.append(el('p', `まだ無い（保存先: ${data.dir}）`, 'saved-empty'));
    return;
  }
  for (const it of data.items) {
    const row = el('div');
    row.className = 'saved-item';
    const g = el('div', '', 'grow');
    g.append(el('span', it.name || '（ビルダー外の顔）'), el('span', ' ' + it.id, 'id'));
    row.append(g);
    if (it.editable) {
      const b = el('button', '編集');
      b.onclick = () => loadSaved(it.id);
      row.append(b);
    } else {
      row.append(el('span', '手書き', 'id'));
    }
    box.append(row);
  }
}

async function loadSaved(id) {
  try {
    state.config = await getJson('/api/load?id=' + encodeURIComponent(id));
  } catch (e) {
    toast(e.message, true);
    return;
  }
  syncIdent();
  renderGrid();
  refresh();
  toast(`${id} を読み込んだ。`);
}

function openFile(e) {
  const file = e.target.files && e.target.files[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = () => {
    let cfg;
    try {
      cfg = JSON.parse(reader.result);
    } catch (err) {
      toast('JSON として読めません: ' + err.message, true);
      return;
    }
    if (!cfg || typeof cfg !== 'object' || !cfg.parts) {
      toast('キャラクターの設定ではないようです（parts がありません）。', true);
      return;
    }
    state.config = cfg;
    syncIdent();
    renderGrid();
    refresh();
    toast(`${file.name} を読み込んだ。`);
  };
  reader.readAsText(file);
  e.target.value = '';
}

// ---------------------------------------------------------------------------
// 書き出し
// ---------------------------------------------------------------------------

function download(name, text, type) {
  const url = URL.createObjectURL(new Blob([text], { type: type + ';charset=utf-8' }));
  const a = el('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** SVG をラスタライズして PNG に。外部参照を持たない SVG なので canvas は汚れない。 */
function downloadPng() {
  const svg = state.mainSvg;
  if (!svg) return;
  const doc = new DOMParser().parseFromString(svg, 'image/svg+xml');
  const root = doc.documentElement;
  const w = parseFloat(root.getAttribute('width')) || 100;
  const h = parseFloat(root.getAttribute('height')) || 100;
  const scale = 8; // 顔は数十 pt しかないので、そのままだと粗い

  const url = URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml;charset=utf-8' }));
  const img = new Image();
  img.onload = () => {
    const canvas = el('canvas');
    canvas.width = Math.round(w * scale);
    canvas.height = Math.round(h * scale);
    const ctx = canvas.getContext('2d');
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    URL.revokeObjectURL(url);
    try {
      canvas.toBlob((blob) => {
        const u = URL.createObjectURL(blob);
        const a = el('a');
        a.href = u;
        a.download = `${safeName()}.png`;
        a.click();
        setTimeout(() => URL.revokeObjectURL(u), 1000);
      }, 'image/png');
    } catch (e) {
      toast('PNG にできませんでした（SVG で書き出してください）: ' + e.message, true);
    }
  };
  img.onerror = () => {
    URL.revokeObjectURL(url);
    toast('PNG にできませんでした（SVG で書き出してください）。', true);
  };
  img.src = url;
}

function safeName() {
  const id = (state.config.id || 'face').trim();
  return /^[a-z0-9][a-z0-9-]*$/.test(id) ? id : 'face';
}

// ---------------------------------------------------------------------------
// 小物
// ---------------------------------------------------------------------------

function el(tag, text, cls) {
  const e = document.createElement(tag);
  if (text) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

function clone(v) {
  return JSON.parse(JSON.stringify(v));
}

function fmt(v) {
  return (Math.round(v * 100) / 100).toString();
}

let toastTimer = null;
function toast(message, bad) {
  const t = $('#toast');
  t.textContent = message;
  t.className = 'show' + (bad ? ' bad' : '');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.className = ''), bad ? 7000 : 3500);
}
