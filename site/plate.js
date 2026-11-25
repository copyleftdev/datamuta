import * as THREE from "three";

/* ------------------------------------------------------------------ setup */

const COL = {
  paper: 0xf3f4f1,
  top: 0xfbfbf9,
  left: 0xeeefeb,
  right: 0xe0e2dc,
  shadow: 0xe6e7e3,
  ink: 0x2a2e33,
  hair: 0xb4b9b1,
  dot: 0xc9ccc5,
  accent: 0x2546c7,
  wash: 0xdfe5fa,
  ok: 0x13795b,
};
const CSS = { ink: "#1B1E21", ink2: "#4A5056", ink3: "#8A9096", hair: "#C7CBC4", accent: "#2546C7", face: "#FBFBF9", left: "#EEEFEB" };

const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)");
const canvas = document.getElementById("plate");
const labelLayer = document.getElementById("labels");
const stage = document.querySelector(".stage");
const heroEl = document.getElementById("hero");
const storyEl = document.getElementById("story");
const stepEls = [...document.querySelectorAll(".step")];
const railBtns = [...document.querySelectorAll(".rail button")];
const bar = document.querySelector(".bar");

let renderer;
try {
  renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
} catch {
  canvas.remove();
}

const scene = new THREE.Scene();
scene.background = new THREE.Color(COL.paper);

const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 400);
const VIEW = new THREE.Vector3(1, 1, 1).normalize();
const RIGHT = new THREE.Vector3(1, 0, -1).normalize();
const UP = new THREE.Vector3(-1, 2, -1).normalize();
const PX = 100; // css px per world unit at zoom 1

const smooth = (a, b, x) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
};
const clamp01 = (x) => Math.min(1, Math.max(0, x));
const easeOut = (t) => 1 - Math.pow(1 - clamp01(t), 3);

/* ------------------------------------------------------------- materials */

const fadeables = []; // [{ mat, base, group }]
function track(mat, group) {
  mat.transparent = true;
  mat.userData.base = mat.opacity;
  fadeables.push({ mat, group });
  return mat;
}
function faceMat(color, group, map) {
  return track(
    new THREE.MeshBasicMaterial({ color: map ? 0xffffff : color, map: map || null, polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1 }),
    group,
  );
}
function lineMat(color, group, opacity = 1) {
  return track(new THREE.LineBasicMaterial({ color, opacity }), group);
}

function canvasTex(w, h, draw) {
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  const g = c.getContext("2d");
  draw(g, w, h);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.anisotropy = renderer ? renderer.capabilities.getMaxAnisotropy() : 1;
  return t;
}

/* box resting on y=0, faces: +x right, +y top, +z left (as seen from the camera) */
function box(w, h, d, tag, { top = COL.top, left = COL.left, right = COL.right, topMap, leftMap, rightMap } = {}) {
  const g = new THREE.Group();
  const geo = new THREE.BoxGeometry(w, h, d);
  geo.translate(0, h / 2, 0);
  const mats = [
    faceMat(right, tag, rightMap),
    faceMat(right, tag),
    faceMat(top, tag, topMap),
    faceMat(top, tag),
    faceMat(left, tag, leftMap),
    faceMat(left, tag),
  ];
  const mesh = new THREE.Mesh(geo, mats);
  const edges = new THREE.LineSegments(new THREE.EdgesGeometry(geo), lineMat(COL.ink, tag));
  g.add(mesh, edges);
  g.userData = { mesh, edges, mats, w, h, d };
  return g;
}

/* ----------------------------------------------------------------- iconography */

function strokeIcon(g, w, h, kind) {
  g.fillStyle = CSS.left;
  g.fillRect(0, 0, w, h);
  g.save();
  g.translate(w / 2, h / 2);
  const s = h / 72;
  g.scale(s, s);
  g.strokeStyle = CSS.ink2;
  g.lineWidth = 3.6;
  g.lineCap = "round";
  g.lineJoin = "round";
  g.beginPath();
  switch (kind) {
    case "capture": // cylinder
      g.ellipse(0, -18, 22, 8, 0, 0, Math.PI * 2);
      g.moveTo(-22, -18); g.lineTo(-22, 18);
      g.moveTo(22, -18); g.lineTo(22, 18);
      g.moveTo(-22, 18); g.ellipse(0, 18, 22, 8, 0, Math.PI, 0, true);
      g.moveTo(-22, 0); g.ellipse(0, 0, 22, 8, 0, Math.PI, 0, true);
      break;
    case "map": // field arrows
      for (const y of [-16, 0, 16]) {
        g.moveTo(-26, y); g.lineTo(-12, y);
        g.moveTo(-4, y); g.lineTo(22, y);
        g.moveTo(15, y - 6); g.lineTo(22, y); g.lineTo(15, y + 6);
      }
      break;
    case "seal": // partition grid
      for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) g.rect(-25 + i * 18, -25 + j * 18, 13, 13);
      break;
    case "approve": // hash in a seal
      g.arc(0, 0, 26, 0, Math.PI * 2);
      g.moveTo(-7, -14); g.lineTo(-10, 14);
      g.moveTo(9, -14); g.lineTo(6, 14);
      g.moveTo(-15, -5); g.lineTo(15, -5);
      g.moveTo(-16, 6); g.lineTo(14, 6);
      break;
    case "publish": // arrow into tray
      g.moveTo(0, -26); g.lineTo(0, 8);
      g.moveTo(-10, -2); g.lineTo(0, 8); g.lineTo(10, -2);
      g.moveTo(-24, 6); g.lineTo(-24, 22); g.lineTo(24, 22); g.lineTo(24, 6);
      break;
    case "verify": // check in frame
      g.rect(-24, -24, 48, 48);
      g.moveTo(-12, 1); g.lineTo(-3, 10); g.lineTo(13, -9);
      break;
  }
  g.stroke();
  g.restore();
}

function screenTex(kind) {
  return canvasTex(640, 440, (g, w, h) => {
    g.fillStyle = CSS.face;
    g.fillRect(0, 0, w, h);
    g.strokeStyle = CSS.hair;
    g.lineWidth = 3;
    g.strokeRect(22, 22, w - 44, h - 44);
    g.fillStyle = "#D5D8D1";
    g.fillRect(22, 22, w - 44, 40);
    for (let i = 0; i < 3; i++) {
      g.beginPath();
      g.arc(48 + i * 26, 42, 7, 0, Math.PI * 2);
      g.fillStyle = "#B9BDB5";
      g.fill();
    }
    g.strokeStyle = "#B4B9B1";
    g.lineWidth = 6;
    g.lineCap = "round";
    if (kind === "table") {
      for (let r = 0; r < 5; r++) {
        const y = 104 + r * 58;
        g.beginPath();
        g.moveTo(54, y); g.lineTo(140, y);
        g.moveTo(180, y); g.lineTo(r % 2 ? 300 : 360, y);
        g.moveTo(420, y); g.lineTo(560, y);
        g.stroke();
        g.lineWidth = 2;
        g.beginPath();
        g.moveTo(44, y + 29); g.lineTo(w - 44, y + 29);
        g.stroke();
        g.lineWidth = 6;
      }
    } else {
      for (let c = 0; c < 2; c++) {
        const x = 54 + c * 280;
        g.lineWidth = 3;
        g.strokeRect(x, 96, 250, 290);
        g.lineWidth = 6;
        for (let r = 0; r < 5; r++) {
          g.beginPath();
          g.moveTo(x + 26, 140 + r * 48);
          g.lineTo(x + (r % 2 ? 150 : 210), 140 + r * 48);
          g.stroke();
        }
      }
    }
  });
}

/* ------------------------------------------------------------------ world */

const world = new THREE.Group();
scene.add(world);
const S2 = Math.SQRT1_2;

/* dot grid on the ground */
{
  const pts = [];
  for (let x = -14; x <= 14; x++) for (let z = -14; z <= 14; z++) pts.push(x, 0, z);
  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
  const m = track(new THREE.PointsMaterial({ color: COL.dot, size: 2, sizeAttenuation: false }), "ground");
  world.add(new THREE.Points(geo, m));
}

function shadow(w, d, x, z, tag, dx = 0.55, dz = 0.55) {
  const geo = new THREE.PlaneGeometry(w, d);
  geo.rotateX(-Math.PI / 2);
  const m = faceMat(COL.shadow, tag);
  const mesh = new THREE.Mesh(geo, m);
  mesh.position.set(x + dx, 0.002, z + dz);
  mesh.renderOrder = -1;
  return mesh;
}

/* --- the runtime stack --- */
const STAGES = ["capture", "map", "seal", "approve", "publish", "verify"];
const SLAB = { w: 3.2, h: 0.46, d: 3.2 };
const CAP_H = 0.3;
const stack = new THREE.Group();
world.add(stack);
stack.add(shadow(4.6, 4.6, 0, 0, "stack", 0.7, 0.7));

const capTex = canvasTex(1024, 1024, (g, w, h) => {
  g.fillStyle = CSS.face;
  g.fillRect(0, 0, w, h);
  g.save();
  g.translate(w / 2, h / 2);
  g.fillStyle = CSS.ink;
  g.font = '600 150px "Schibsted Grotesk", sans-serif';
  g.textAlign = "center";
  g.textBaseline = "middle";
  g.fillText("datamuta", 0, 0);
  g.fillStyle = CSS.accent;
  g.fillRect(-300, 92, 120, 14);
  g.restore();
});
const cap = box(SLAB.w, CAP_H, SLAB.d, "stack", { topMap: capTex });
stack.add(cap);

const slabs = STAGES.map((k) => {
  const tex = canvasTex(1400, 200, (g, w, h) => strokeIcon(g, w, h, k));
  const s = box(SLAB.w, SLAB.h, SLAB.d, "stack", { leftMap: tex });
  stack.add(s);
  return s;
});

/* partition cells on the seal slab (36 = an illustrative 6×6 spool) */
const CELLS = 36;
const cellGeo = new THREE.BoxGeometry(0.34, 0.06, 0.34);
cellGeo.translate(0, 0.03, 0);
const cellMat = new THREE.MeshBasicMaterial({ color: 0xffffff, transparent: true });
const cells = new THREE.InstancedMesh(cellGeo, cellMat, CELLS);
{
  const m = new THREE.Matrix4();
  for (let i = 0; i < CELLS; i++) {
    const cx = (i % 6) - 2.5;
    const cz = Math.floor(i / 6) - 2.5;
    m.makeTranslation(cx * 0.46, SLAB.h, cz * 0.46);
    cells.setMatrixAt(i, m);
    cells.setColorAt(i, new THREE.Color(COL.right));
  }
}
slabs[2].add(cells);

/* --- source: a database drum --- */
function drum(x, z, tag) {
  const g = new THREE.Group();
  const R = 1.05, H = 0.34, GAP = 0.1;
  for (let i = 0; i < 3; i++) {
    const geo = new THREE.CylinderGeometry(R, R, H, 64, 1);
    geo.translate(0, H / 2 + i * (H + GAP), 0);
    const mats = [faceMat(COL.left, tag), faceMat(COL.top, tag), faceMat(COL.top, tag)];
    const mesh = new THREE.Mesh(geo, mats);
    const rims = new THREE.LineSegments(new THREE.EdgesGeometry(geo, 30), lineMat(COL.ink, tag));
    // silhouette verticals for the iso view
    const y0 = i * (H + GAP), y1 = y0 + H;
    const sx = R * S2, sz = -R * S2;
    const sil = new THREE.BufferGeometry().setFromPoints([
      new THREE.Vector3(sx, y0, sz), new THREE.Vector3(sx, y1, sz),
      new THREE.Vector3(-sx, y0, -sz), new THREE.Vector3(-sx, y1, -sz),
    ]);
    g.add(mesh, rims, new THREE.LineSegments(sil, lineMat(COL.ink, tag)));
  }
  g.add(shadow(2.2, 2.2, 0, 0, tag, 0.45, 0.45));
  g.position.set(x, 0, z);
  return g;
}
const SRC = new THREE.Vector3(0, 0, 7.4);
const source = drum(SRC.x, SRC.z, "source");
world.add(source);

/* --- targets: standing panels on bases --- */
function device(x, z, kind, tag) {
  const g = new THREE.Group();
  const base = box(2.5, 0.14, 1.3, tag);
  const tex = screenTex(kind);
  const panel = box(2.3, 1.56, 0.12, tag, { leftMap: tex });
  panel.position.set(0, 0.14, -0.35);
  const foot = box(0.5, 0.18, 0.3, tag);
  foot.position.set(0, 0.14, -0.2);
  g.add(shadow(2.9, 1.7, 0, 0, tag, 0.35, 0.35), base, foot, panel);
  g.position.set(x, 0, z);
  g.userData.panel = panel;
  return g;
}
const TARGETS = [
  { key: "pg", name: "PostgreSQL", sub: "documented example", x: 0, z: -7.2, kind: "table", lab: false },
  { key: "my", name: "MySQL", sub: "lab profile", x: 5.6, z: -5.2, kind: "table", lab: true },
  { key: "mg", name: "MongoDB", sub: "lab replica set", x: 7.4, z: -1.4, kind: "docs", lab: true },
  { key: "fs", name: "Firestore", sub: "emulator only", x: 6.6, z: 3.4, kind: "docs", lab: true },
];
for (const t of TARGETS) {
  t.group = device(t.x, t.z, t.kind, "targets");
  world.add(t.group);
}

/* --- routes on the ground --- */
function polyline(points) {
  const v = points.map((p) => new THREE.Vector3(p[0], 0.012, p[1]));
  const lens = [0];
  for (let i = 1; i < v.length; i++) lens.push(lens[i - 1] + v[i].distanceTo(v[i - 1]));
  return { v, lens, total: lens[lens.length - 1] };
}
function along(path, t, out) {
  const d = clamp01(t) * path.total;
  let i = 1;
  while (i < path.lens.length - 1 && path.lens[i] < d) i++;
  const a = path.v[i - 1], b = path.v[i];
  const f = (d - path.lens[i - 1]) / (path.lens[i] - path.lens[i - 1] || 1);
  return out.copy(a).lerp(b, f);
}
function drawRoute(path, dashed) {
  const geo = new THREE.BufferGeometry().setFromPoints(path.v);
  const mat = dashed
    ? track(new THREE.LineDashedMaterial({ color: COL.hair, dashSize: 0.18, gapSize: 0.14 }), "routes")
    : track(new THREE.LineBasicMaterial({ color: 0x9da29a }), "routes");
  const line = new THREE.Line(geo, mat);
  if (dashed) line.computeLineDistances();
  world.add(line);
}
const H2 = SLAB.w / 2;
const inRoute = polyline([[0, SRC.z - 1.15], [0, H2 + 0.02]]);
drawRoute(inRoute, false);
for (const t of TARGETS) {
  let pts;
  if (t.key === "pg") pts = [[0, -H2 - 0.02], [0, t.z + 0.72]];
  else if (t.z < 0) pts = [[H2 + 0.02, 0], [t.x, 0], [t.x, t.z + 0.72]];
  else pts = [[H2 + 0.02, 0], [t.x, 0], [t.x, t.z - 0.72]];
  t.route = polyline(pts);
  drawRoute(t.route, t.lab);
}

/* --- packets (partitions in flight) --- */
const packetGeo = new THREE.BoxGeometry(0.42, 0.12, 0.3);
packetGeo.translate(0, 0.06, 0);
const packetEdge = new THREE.EdgesGeometry(packetGeo);
function makePacket(lab) {
  const g = new THREE.Group();
  const m = new THREE.MeshBasicMaterial({ color: lab ? 0x8fa0e3 : COL.accent, transparent: true });
  const l = new THREE.LineBasicMaterial({ color: lab ? 0x6377c9 : 0x162f8f, transparent: true });
  g.add(new THREE.Mesh(packetGeo, m), new THREE.LineSegments(packetEdge, l));
  g.userData.mats = [m, l];
  g.visible = false;
  world.add(g);
  return g;
}
const inPackets = [0, 1, 2].map(() => makePacket(false));
const outPackets = [];
for (const t of TARGETS) {
  const n = t.lab ? 1 : 3;
  for (let i = 0; i < n; i++) outPackets.push({ t, i, g: makePacket(t.lab) });
}

/* ------------------------------------------------------------------ labels */

function label(text, sub, side = "l") {
  const el = document.createElement("div");
  el.className = "lbl" + (side === "r" ? " r" : "");
  el.innerHTML = `<span class="t"><span></span><small></small></span>`;
  el.querySelector(".t span").textContent = text;
  el.querySelector("small").textContent = sub;
  labelLayer.appendChild(el);
  return el;
}
const heroLabels = [
  { el: label("Source", "PostgreSQL 17 · orders", "r"), at: () => new THREE.Vector3(SRC.x + 0.95, 0.55, SRC.z - 0.95) },
  ...TARGETS.map((t) => ({ el: label(t.name, t.sub, "r"), at: () => new THREE.Vector3(t.x + 1.3, 0.9, t.z - 0.7), t })),
];
const pgLabel = heroLabels.find((l) => l.t && l.t.key === "pg");
const SLAB_SUB = ["capture --spool", "mapping.json", "sealed spool · merkle", "--approved-plan-sha256", "records + receipt + cursor", "verify --out"];
const slabLabels = slabs.map((s, i) => ({
  el: Object.assign(label(STAGES[i], SLAB_SUB[i], "r"), { className: "lbl r slab" }),
  at: () => {
    const p = new THREE.Vector3(SLAB.w / 2 + 0.15, SLAB.h * 0.5, -SLAB.d / 2 - 0.15);
    return s.localToWorld(p);
  },
}));

/* ------------------------------------------------------------------ layout */

let W = 1, H = 1;
const st = { scroll: 0, e: 0, eTarget: 0, step: -1 };

function stackLayout(e) {
  const gap = 0.06 + e * 0.92;
  let y = 0;
  for (let i = slabs.length - 1; i >= 0; i--) {
    slabs[i].position.y = y;
    y += SLAB.h + gap;
  }
  cap.position.y = y;
  return y + CAP_H;
}

function boundsOf(points) {
  let r0 = Infinity, r1 = -Infinity, u0 = Infinity, u1 = -Infinity;
  for (const p of points) {
    const r = p.dot(RIGHT), u = p.dot(UP);
    r0 = Math.min(r0, r); r1 = Math.max(r1, r);
    u0 = Math.min(u0, u); u1 = Math.max(u1, u);
  }
  return { r0, r1, u0, u1 };
}
function boxPoints(x0, x1, y0, y1, z0, z1) {
  const out = [];
  for (const x of [x0, x1]) for (const y of [y0, y1]) for (const z of [z0, z1]) out.push(new THREE.Vector3(x, y, z));
  return out;
}
const heroPts = [
  ...boxPoints(-2.4, 2.4, 0, 3.6, -2.4, 2.4),
  ...boxPoints(-1.1, 1.1, 0, 1.3, SRC.z - 1.1, SRC.z + 1.6),
  ...TARGETS.flatMap((t) => boxPoints(t.x - 1.3, t.x + 1.5, 0, 1.75, t.z - 0.7, t.z + 0.9)),
];
const heroB = boundsOf(heroPts);

function frame(b, rect) {
  const zoom = Math.min(rect.w / ((b.r1 - b.r0) * PX), rect.h / ((b.u1 - b.u0) * PX));
  const mr = (b.r0 + b.r1) / 2, mu = (b.u0 + b.u1) / 2;
  // rect centre relative to canvas centre, +y up
  const rx = rect.x + rect.w / 2 - W / 2;
  const ry = H / 2 - (rect.y + rect.h / 2);
  return { zoom, tr: mr - rx / (PX * zoom), tu: mu - ry / (PX * zoom) };
}
function rects() {
  const narrow = W < 900;
  if (narrow) {
    const heroTop = heroEl.getBoundingClientRect().bottom + 28;
    return {
      hero: { x: 12, y: Math.min(heroTop, H * 0.62), w: W - 140, h: Math.max(H - Math.min(heroTop, H * 0.62) - 20, H * 0.3) },
      story: { x: W * 0.06, y: 150, w: W * 0.6, h: H * 0.4 },
    };
  }
  return {
    hero: { x: Math.max(W * 0.42, 540), y: 100, w: W - Math.max(W * 0.42, 540) - 200, h: H - 130 },
    heroWide: null,
    story: { x: W * 0.5, y: 150, w: W * 0.3, h: H - 220 },
  };
}

function resize() {
  W = canvas.clientWidth || innerWidth;
  H = canvas.clientHeight || innerHeight;
  if (!renderer) return;
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
  renderer.setSize(W, H, false);
  camera.left = -W / 2 / PX;
  camera.right = W / 2 / PX;
  camera.top = H / 2 / PX;
  camera.bottom = -H / 2 / PX;
  camera.updateProjectionMatrix();
}

/* ------------------------------------------------------------------ scroll */

const P = { heroEnd: 0.07, explodeEnd: 0.19, steps: 6 };
function stageProgress() {
  const r = stage.getBoundingClientRect();
  const span = r.height - H;
  return span > 0 ? clamp01(-r.top / span) : 0;
}
function stepAt(p) {
  if (p < P.explodeEnd - 0.02) return -1;
  return Math.min(P.steps - 1, Math.floor((p - P.explodeEnd + 0.02) / ((1 - P.explodeEnd) / P.steps)));
}
railBtns.forEach((b) =>
  b.addEventListener("click", () => {
    const i = +b.dataset.step;
    const r = stage.getBoundingClientRect();
    const span = r.height - H;
    const p = P.explodeEnd + (i + 0.35) * ((1 - P.explodeEnd) / P.steps);
    scrollTo({ top: scrollY + r.top + p * span, behavior: reduceMotion.matches ? "auto" : "smooth" });
  }),
);

/* ------------------------------------------------------------------ animation */

const tmp = new THREE.Vector3();
const proj = new THREE.Vector3();
const CYCLE = 10;
const activeEdge = new THREE.Color(COL.accent);
const inkEdge = new THREE.Color(COL.ink);
const topBase = new THREE.Color(COL.top);
const topWash = new THREE.Color(COL.wash);

function setGroupOpacity(tag, a) {
  for (const f of fadeables) if (f.group === tag) f.mat.opacity = f.mat.userData.base * a;
}

function placeLabel(l, alpha, extraClass) {
  if (alpha <= 0.01) {
    l.el.style.opacity = "0";
    return;
  }
  proj.copy(l.at()).project(camera);
  const x = (proj.x * 0.5 + 0.5) * W;
  const y = (-proj.y * 0.5 + 0.5) * H;
  const right = l.el.classList.contains("r");
  l.el.style.transform = `translate(${right ? x : x - l.el.offsetWidth}px, ${y - l.el.offsetHeight / 2}px)`;
  l.el.style.opacity = String(alpha);
}

function heroCycle(t, a) {
  // t: seconds within cycle; a: overall visibility of the flow (0..1)
  const show = a > 0.02;
  inPackets.forEach((g, i) => {
    const k = (t - i * 0.45) / 1.7;
    g.visible = show && k > 0 && k < 1;
    if (g.visible) {
      along(inRoute, easeOut(k), g.position);
      g.rotation.y = 0;
      g.userData.mats.forEach((m) => (m.opacity = a));
    }
  });
  // slabs light up top to bottom as the batch passes through
  const pass = (t - 2.4) / 0.32;
  slabs.forEach((s, i) => {
    const on = show && pass >= i && pass < i + 1.6;
    s.userData.edges.material.color.copy(on ? activeEdge : inkEdge);
    s.userData.mats[2].color.copy(on ? topWash : topBase);
  });
  outPackets.forEach(({ t: tg, i, g }) => {
    const start = 4.6 + i * 0.42 + (tg.lab ? 0.25 : 0);
    const k = (t - start) / 1.9;
    g.visible = show && k > 0 && k < 1;
    if (g.visible) {
      along(tg.route, easeOut(k), g.position);
      g.userData.mats.forEach((m) => (m.opacity = a * (tg.lab ? 0.8 : 1)));
    }
  });
  const done = t > 7.2;
  pgLabel.el.classList.toggle("ok", done && show);
  pgLabel.el.querySelector("small").textContent = done && show ? "3 rows · 12 fields verified" : "documented example";
}

function storyState(step, local) {
  slabs.forEach((s, i) => {
    const on = i === step;
    s.userData.edges.material.color.copy(on ? activeEdge : inkEdge);
    s.userData.mats[2].color.copy(on ? topWash : topBase);
  });
  // seal: fill the partition cells while that step is on screen
  const filled = step > 2 ? CELLS : step === 2 ? Math.floor(clamp01(local * 1.4) * CELLS) : 0;
  const cFill = new THREE.Color(COL.accent), cIdle = new THREE.Color(COL.right), cDone = new THREE.Color(0x8fa0e3);
  for (let i = 0; i < CELLS; i++) cells.setColorAt(i, i < filled ? (step === 2 ? cFill : cDone) : cIdle);
  cells.instanceColor.needsUpdate = true;
}

const timer = new THREE.Timer();
timer.connect(document);
let running = true;
let lastStep = -2;

function tick() {
  timer.update();
  const now = timer.getElapsed();
  const p = stageProgress();
  const e = smooth(P.heroEnd, P.explodeEnd, p);
  const heroA = 1 - smooth(0.01, P.heroEnd, p);
  const storyA = smooth(P.explodeEnd - 0.04, P.explodeEnd, p);
  const step = stepAt(p);
  const stepSpan = (1 - P.explodeEnd) / P.steps;
  const local = step >= 0 ? (p - P.explodeEnd - step * stepSpan) / stepSpan : 0;

  bar.classList.toggle("ruled", scrollY > 8);

  // DOM layers
  heroEl.style.opacity = String(heroA);
  heroEl.style.transform = `translateY(${-24 * (1 - heroA)}px)`;
  heroEl.style.visibility = heroA < 0.02 ? "hidden" : "visible";
  storyEl.style.opacity = String(storyA);
  storyEl.classList.toggle("live", storyA > 0.5);
  if (step !== lastStep) {
    stepEls.forEach((el, i) => el.classList.toggle("on", i === step));
    railBtns.forEach((b, i) => (i === step ? b.setAttribute("aria-current", "step") : b.removeAttribute("aria-current")));
    lastStep = step;
  }

  if (!renderer) return;

  // stack explode + camera
  const top = stackLayout(e);
  const R = rects();
  const fa = frame(heroB, R.hero);
  const storyB = boundsOf(boxPoints(-1.7, 1.7, 0, top, -1.7, 1.7));
  const fb = frame(storyB, R.story);
  const k = e;
  camera.zoom = THREE.MathUtils.lerp(fa.zoom, fb.zoom, k);
  const tr = THREE.MathUtils.lerp(fa.tr, fb.tr, k);
  const tu = THREE.MathUtils.lerp(fa.tu, fb.tu, k);
  tmp.copy(RIGHT).multiplyScalar(tr).addScaledVector(UP, tu);
  camera.position.copy(tmp).addScaledVector(VIEW, 60);
  camera.lookAt(tmp);
  camera.updateProjectionMatrix();

  // surroundings recede as the stack opens
  const around = 1 - 0.96 * e;
  setGroupOpacity("source", around);
  setGroupOpacity("targets", around);
  setGroupOpacity("routes", around);
  setGroupOpacity("ground", 1 - 0.5 * e);

  const motion = !reduceMotion.matches;
  const t = motion ? now % CYCLE : 8.5;
  if (e < 0.5) heroCycle(t, 1 - smooth(0, 0.5, e));
  else {
    inPackets.forEach((g) => (g.visible = false));
    outPackets.forEach((o) => (o.g.visible = false));
    storyState(step, local);
  }

  const narrow = W < 900;
  for (const l of heroLabels) placeLabel(l, heroA * (1 - e) * (narrow && l !== pgLabel ? 0 : 1));
  slabLabels.forEach((l, i) => {
    placeLabel(l, smooth(0.6, 1, e) * (i === step ? 1 : 0.55));
    l.el.classList.toggle("on", i === step);
  });

  renderer.render(scene, camera);
}

/* run only while the stage is on screen */
const io = new IntersectionObserver(([en]) => {
  running = en.isIntersecting;
  if (renderer) renderer.setAnimationLoop(running ? tick : null);
});

async function start() {
  try {
    await Promise.race([document.fonts.ready, new Promise((r) => setTimeout(r, 1500))]);
  } catch {}
  resize();
  addEventListener("resize", resize);
  if (!renderer) {
    addEventListener("scroll", tick, { passive: true });
    tick();
    return;
  }
  io.observe(stage);
  addEventListener("scroll", () => { if (!running) tick(); }, { passive: true });
  tick();
}
start();

/* ------------------------------------------------------------------ copy buttons */
document.querySelectorAll(".copy-btn").forEach((b) =>
  b.addEventListener("click", async () => {
    const src = document.getElementById(b.dataset.copy);
    try {
      await navigator.clipboard.writeText(src.innerText.trim());
      b.textContent = "Copied";
      b.classList.add("done");
    } catch {
      b.textContent = "Select to copy";
    }
    setTimeout(() => { b.textContent = "Copy"; b.classList.remove("done"); }, 1600);
  }),
);

/* ------------------------------------------------------------------ section reveals */
if (!reduceMotion.matches && "IntersectionObserver" in window) {
  document.documentElement.classList.add("kinetic");
  document.querySelectorAll(".map, .reveal").forEach((el) => {
    [...el.children].forEach((c, i) => c.style.setProperty("--i", i));
  });
  const rio = new IntersectionObserver(
    (ents) => ents.forEach((en) => { if (en.isIntersecting) { en.target.classList.add("in"); rio.unobserve(en.target); } }),
    { rootMargin: "0px 0px -12% 0px" },
  );
  document.querySelectorAll(".map, .reveal").forEach((el) => rio.observe(el));
}
