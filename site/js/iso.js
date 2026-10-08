/* Isometric objects as inline SVG.
   World space: x to the right-back, y to the left-back, z up. Shapes are
   drawn in plan (x, y), extruded between two heights, and flat artwork is
   projected onto any face with an affine matrix. */
(function () {
  'use strict';

  const C = Math.cos(Math.PI / 6);
  const S = 0.5;
  const P = (x, y, z) => [(x - y) * C, (x + y) * S - z];
  const f = (n) => Math.round(n * 100) / 100;
  const pts = (a) => a.map((p) => f(p[0]) + ',' + f(p[1])).join(' ');
  let uid = 0;
  const id = (p) => `${p}${++uid}`;

  // ---------------------------------------------------------------- shapes

  function rrect(x, y, w, d, r, n = 6) {
    r = Math.max(0.01, Math.min(r, w / 2, d / 2));
    const out = [];
    const corners = [
      [x + w - r, y + r, -90],
      [x + w - r, y + d - r, 0],
      [x + r, y + d - r, 90],
      [x + r, y + r, 180],
    ];
    for (const [cx, cy, a0] of corners) {
      for (let i = 0; i <= n; i++) {
        const a = ((a0 + (90 * i) / n) * Math.PI) / 180;
        out.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
      }
    }
    return out;
  }

  function circle(cx, cy, r, n = 44) {
    const out = [];
    for (let i = 0; i < n; i++) {
      const a = (i / n) * Math.PI * 2;
      out.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
    }
    return out;
  }

  function rot(shape, deg, cx = 0, cy = 0) {
    const a = (deg * Math.PI) / 180;
    const c = Math.cos(a);
    const s = Math.sin(a);
    return shape.map(([x, y]) => [cx + (x - cx) * c - (y - cy) * s, cy + (x - cx) * s + (y - cy) * c]);
  }

  const move = (shape, dx, dy) => shape.map(([x, y]) => [x + dx, y + dy]);

  function hull(points) {
    const p = points.slice().sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    const cross = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    const lo = [];
    for (const q of p) {
      while (lo.length >= 2 && cross(lo[lo.length - 2], lo[lo.length - 1], q) <= 0) lo.pop();
      lo.push(q);
    }
    const up = [];
    for (let i = p.length - 1; i >= 0; i--) {
      const q = p[i];
      while (up.length >= 2 && cross(up[up.length - 2], up[up.length - 1], q) <= 0) up.pop();
      up.push(q);
    }
    up.pop();
    lo.pop();
    return lo.concat(up);
  }

  // ------------------------------------------------------------- materials

  const M = {
    ceramic: { top: '#EEE8DC', l: '#D6CDBD', r: '#A99E8B', edge: 'rgba(255,255,255,.7)' },
    stone: { top: '#DCD3C3', l: '#C4B9A7', r: '#968A76', edge: 'rgba(255,255,255,.45)' },
    paper: { top: '#FCFAF5', l: '#E7E0D2', r: '#C9BFAD', edge: '#fff' },
    graphite: { top: '#3C3631', l: '#2D2824', r: '#201C19', edge: 'rgba(255,255,255,.1)' },
    black: { top: '#0C0C0C', l: '#1A1816', r: '#070707', edge: 'rgba(255,255,255,.16)' },
  };
  const tint = (hex, l, r) => ({ top: hex, l, r, edge: 'rgba(255,255,255,.35)' });

  // ------------------------------------------------------------ primitives

  /** Extrude a convex plan shape from z0 to z1. `soft` widens the light
      falloff across the front corner (use the corner radius). */
  function prism(shape, z0, z1, m, soft = 0) {
    const top = shape.map(([x, y]) => P(x, y, z1));
    const bot = shape.map(([x, y]) => P(x, y, z0));
    const h = hull(top.concat(bot));
    let front = shape[0];
    for (const p of shape) if (p[0] + p[1] > front[0] + front[1]) front = p;
    const fx = (front[0] - front[1]) * C;
    const s = Math.max(0.6, soft * C);
    const g = id('g');
    return (
      `<linearGradient id="${g}" gradientUnits="userSpaceOnUse" x1="${f(fx - s)}" y1="0" x2="${f(fx + s)}" y2="0">` +
      `<stop offset="0" stop-color="${m.l}"/><stop offset="1" stop-color="${m.r}"/></linearGradient>` +
      `<polygon points="${pts(h)}" fill="url(#${g})"/>` +
      `<polygon points="${pts(top)}" fill="${m.top}"${m.edge ? ` stroke="${m.edge}" stroke-width=".9" stroke-linejoin="round"` : ''}/>`
    );
  }

  const cyl = (cx, cy, r, z0, z1, m) => prism(circle(cx, cy, r), z0, z1, m, r);
  const box = (x, y, w, d, z0, z1, m, r = 0) => prism(rrect(x, y, w, d, r), z0, z1, m, r);

  /** Flat artwork on the horizontal plane at height z (plan coordinates). */
  const onTop = (z, inner, cls = '') =>
    `<g${cls ? ` class="${cls}"` : ''} transform="matrix(${f(C)},${S},${f(-C)},${S},0,${f(-z)})">${inner}</g>`;
  /** Artwork on the face y = y0 (faces front-left). u runs along +x from x0, v runs down from z0. */
  const onLeft = (y0, x0, z0, inner, cls = '') =>
    `<g${cls ? ` class="${cls}"` : ''} transform="matrix(${f(C)},${S},0,1,${f((x0 - y0) * C)},${f((x0 + y0) * S - z0)})">${inner}</g>`;
  /** Artwork on the face x = x0 (faces front-right). u runs along -y from y0, v runs down from z0. */
  const onRight = (x0, y0, z0, inner, cls = '') =>
    `<g${cls ? ` class="${cls}"` : ''} transform="matrix(${f(C)},${-S},0,1,${f((x0 - y0) * C)},${f((x0 + y0) * S - z0)})">${inner}</g>`;

  const shadow = (shape, o = 0.55, dx = 6, dy = 6) =>
    `<polygon points="${pts(move(shape, dx, dy).map(([x, y]) => P(x, y, 0)))}" fill="rgba(0,0,0,${o})" filter="url(#isoSoft)"/>`;

  /** Path for a flat shape list in plan/face coordinates. */
  const poly = (shape, attrs) => `<polygon points="${pts(shape)}" ${attrs}/>`;

  /** Thick flat artwork standing on the y = const plane (stack of copies). */
  function slab(y0, x0, z0, inner, back, depth = 6) {
    let s = '';
    for (let i = depth; i > 0; i--) s += onLeft(y0 - i, x0, z0, inner.replace(/__FILL__/g, back));
    return s;
  }

  const svg = (inner, vb = '-150 -165 300 265', cls = '') =>
    `<svg class="iso ${cls}" viewBox="${vb}" xmlns="http://www.w3.org/2000/svg" aria-hidden="true" focusable="false"><defs></defs>${inner}</svg>`;

  // ------------------------------------------------------------ the objects

  const SEA = '#7FC8B5';
  const HONEY = '#E9B44C';
  const LILAC = '#A897F5';
  const BLUE = '#5AA9F0';
  const GREEN = '#34C759';
  const ORANGE = '#FF9F0A';
  const CLAUDE = '#D97757';

  const objects = {
    media() {
      const base = rrect(-88, -88, 176, 176, 22);
      let s = shadow(base);
      s += prism(base, 0, 20, M.ceramic, 22);
      s += onTop(20, `<circle cx="-64" cy="64" r="5" fill="#C9BFAE"/><circle cx="-48" cy="64" r="5" fill="#C9BFAE"/>`);
      s += cyl(-12, -8, 66, 20, 26, M.graphite);
      s += cyl(-12, -8, 61, 26, 29, M.black);
      let grooves = '';
      for (const r of [56, 50, 44, 38, 32]) grooves += `<circle r="${r}" fill="none" stroke="rgba(255,255,255,.075)" stroke-width="1"/>`;
      s += onTop(
        29,
        `<g transform="translate(-12 -8)">${grooves}
          <path d="M0 0 L40 -42 A58 58 0 0 1 56 -14Z M0 0 L-40 42 A58 58 0 0 1 -56 14Z" fill="rgba(255,255,255,.06)"/>
          <g class="spin-me">
            <circle r="21" fill="${SEA}"/>
            <circle r="21" fill="url(#isoLabel)" opacity=".5"/>
            <path d="M-9 -12 h18 a3 3 0 0 1 0 6 h-18 a3 3 0 0 1 0 -6z" fill="#0b0b0b" opacity=".8"/>
            <rect x="-10" y="5" width="13" height="2.4" rx="1.2" fill="#0b0b0b" opacity=".45"/>
            <circle r="2.6" fill="#EEE8DC"/>
          </g></g>`
      );
      s += cyl(60, -60, 13, 20, 34, M.stone);
      s += cyl(60, -60, 6, 34, 41, M.graphite);
      s += prism(move(rot(rrect(-2.6, 0, 5.2, 74, 2.6), 28), 60, -60), 37, 40, M.paper, 2.6);
      const head = move(rot(rrect(-6, 66, 12, 16, 3), 28), 60, -60);
      s += prism(head, 34, 40, M.graphite, 3);
      return svg(s);
    },

    timer() {
      let s = shadow(circle(0, 0, 74));
      s += cyl(0, 0, 74, 0, 42, M.ceramic);
      s += cyl(0, 0, 66, 42, 45, M.paper);
      let ticks = '';
      for (let i = 0; i < 60; i++) {
        const a = (i / 60) * Math.PI * 2;
        const r0 = i % 5 === 0 ? 50 : 54;
        ticks += `<line x1="${f(Math.cos(a) * r0)}" y1="${f(Math.sin(a) * r0)}" x2="${f(Math.cos(a) * 58)}" y2="${f(Math.sin(a) * 58)}" stroke="rgba(40,34,30,${i % 5 === 0 ? 0.55 : 0.25})" stroke-width="${i % 5 === 0 ? 1.6 : 1}"/>`;
      }
      s += onTop(
        45,
        `${ticks}<circle r="40" fill="none" stroke="rgba(40,34,30,.09)" stroke-width="8"/>
         <circle class="countdown" r="40" fill="none" stroke="${HONEY}" stroke-width="8" stroke-linecap="round"
           stroke-dasharray="251.3" stroke-dashoffset="62" transform="rotate(-90)"/>`
      );
      s += cyl(0, 0, 17, 45, 60, M.graphite);
      s += onTop(60, `<g class="spin-slow"><rect x="-2" y="-14" width="4" height="10" rx="2" fill="${HONEY}"/></g>`);
      return svg(s);
    },

    downloads() {
      const shape = rrect(-64, -64, 128, 128, 10);
      let s = shadow(shape);
      s += prism(shape, 0, 64, M.ceramic, 10);
      const cp = id('c');
      const inner = rrect(-52, -52, 104, 104, 5);
      const rim = inner.map(([x, y]) => P(x, y, 64));
      const d = 40;
      const floor = inner.map(([x, y]) => P(x, y, 64 - d));
      const wallBack = [P(-52, -52, 64), P(52, -52, 64), P(52, -52, 64 - d), P(-52, -52, 64 - d)];
      const wallSide = [P(-52, -52, 64), P(-52, 52, 64), P(-52, 52, 64 - d), P(-52, -52, 64 - d)];
      s +=
        `<clipPath id="${cp}"><polygon points="${pts(rim)}"/></clipPath><g clip-path="url(#${cp})">` +
        poly(floor, 'fill="#B9AE9B"') +
        poly(wallBack, 'fill="#A39883"') +
        poly(wallSide, 'fill="#C8BEAC"') +
        `</g>`;
      s += onLeft(64, -64, 64, `<rect x="22" y="34" width="84" height="7" rx="3.5" fill="rgba(40,34,30,.14)"/>
        <rect class="grow-x" x="22" y="34" width="84" height="7" rx="3.5" fill="${BLUE}"/>`);
      const arrow = `<path d="M-9 0h18v40h16L0 70-25 40h16z" fill="__FILL__"/>`;
      s += `<g class="bob">${slab(0, 0, 170, arrow, '#2E6FA8', 7)}${onLeft(0, 0, 170, arrow.replace('__FILL__', BLUE))}</g>`;
      return svg(s);
    },

    clipboard() {
      const board = rrect(-64, -86, 128, 172, 12);
      let s = shadow(board);
      s += prism(board, 0, 8, M.stone, 12);
      const sheets = [-5, 3, 0];
      sheets.forEach((deg, i) => {
        const sh = rot(rrect(-52, -66, 104, 140, 4), deg);
        const z = 8 + i * 2.4;
        const body = prism(sh, z, z + 1.6, M.paper, 4);
        if (i < 2) s += body;
        else {
          let lines = '';
          const ws = [70, 84, 56, 78, 40];
          ws.forEach((w, j) => (lines += `<rect x="-40" y="${-40 + j * 14}" width="${w}" height="5" rx="2.5" fill="rgba(40,34,30,${j === 2 ? 0 : 0.16})"/>`));
          lines += `<rect x="-40" y="-12" width="56" height="5" rx="2.5" fill="${BLUE}" opacity=".7"/>`;
          lines += `<rect x="-40" y="34" width="44" height="28" rx="4" fill="${SEA}" opacity=".55"/>`;
          s += `<g class="slide">${body}${onTop(z + 1.6, lines)}</g>`;
        }
      });
      s += box(-24, -96, 48, 22, 8, 20, M.graphite, 6);
      s += onTop(20, `<rect x="-10" y="-90" width="20" height="8" rx="4" fill="#1C1916"/>`);
      return svg(s);
    },

    shelf() {
      const tray = rrect(-96, -62, 192, 124, 18);
      let s = shadow(tray);
      s += prism(tray, 0, 14, M.ceramic, 18);
      s += onTop(14, `<path d="${roundPath(-84, -50, 168, 100, 12)}" fill="#DCD3C4"/>`);
      const files = [
        { dx: -48, rot: -12, tag: BLUE, label: 'PNG' },
        { dx: 0, rot: 2, tag: '#F06B5A', label: 'PDF' },
        { dx: 46, rot: 14, tag: '#4CC38A', label: 'XLSX' },
      ];
      files.forEach((fl, i) => {
        const shape = move(rot(rrect(-30, -40, 60, 80, 6), fl.rot), fl.dx, 0);
        const z = 16 + i * 4;
        const art =
          `<g transform="translate(${fl.dx} 0) rotate(${fl.rot})">` +
          `<rect x="-22" y="-32" width="18" height="10" rx="3" fill="${fl.tag}"/>` +
          `<rect x="-22" y="-12" width="40" height="4" rx="2" fill="rgba(40,34,30,.18)"/>` +
          `<rect x="-22" y="-2" width="30" height="4" rx="2" fill="rgba(40,34,30,.18)"/>` +
          `<text x="-22" y="26" font-family="Geist Mono, monospace" font-size="9" font-weight="500" fill="rgba(40,34,30,.55)">${fl.label}</text></g>`;
        s += `<g class="lift lift-${i}">${prism(shape, z, z + 3, M.paper, 6)}${onTop(z + 3, art)}</g>`;
      });
      return svg(s);
    },

    voice() {
      const base = rrect(-70, -62, 170, 124, 22);
      let s = shadow(base);
      s += prism(base, 0, 12, M.ceramic, 22);
      s += cyl(-20, 0, 26, 12, 18, M.stone);
      s += cyl(-20, 0, 5, 18, 58, M.graphite);
      s += cyl(-20, 0, 24, 58, 104, M.black);
      // dome
      const c = P(-20, 0, 104);
      const R = 24 * 1.2247;
      const ry = 24 * 0.7071;
      const dg = id('d');
      s +=
        `<radialGradient id="${dg}" cx=".35" cy=".3" r=".8"><stop offset="0" stop-color="#3a3632"/><stop offset=".55" stop-color="#121110"/><stop offset="1" stop-color="#050505"/></radialGradient>` +
        `<path d="M${f(c[0] - R)} ${f(c[1])} A${f(R)} ${f(R)} 0 0 1 ${f(c[0] + R)} ${f(c[1])} A${f(R)} ${f(ry)} 0 0 1 ${f(c[0] - R)} ${f(c[1])}Z" fill="url(#${dg})"/>` +
        `<path d="M${f(c[0] - R)} ${f(c[1])} A${f(R)} ${f(R)} 0 0 1 ${f(c[0] + R)} ${f(c[1])} A${f(R)} ${f(ry)} 0 0 1 ${f(c[0] - R)} ${f(c[1])}Z" fill="url(#isoMesh)" opacity=".7"/>`;
      // band
      const b0 = P(-20, 0, 74);
      s += `<ellipse cx="${f(b0[0])}" cy="${f(b0[1])}" rx="${f(R)}" ry="${f(ry)}" fill="none" stroke="rgba(255,255,255,.14)" stroke-width="1.2" stroke-dasharray="0 ${f(R * 1.2)} ${f(R * 3)}"/>`;
      // level bars
      const hs = [18, 34, 52, 30, 44, 22, 12];
      hs.forEach((h, i) => {
        s += `<g class="lvl lvl-${i}">${box(30 + i * 9, -4, 5, 8, 12, 12 + h, tint(SEA, '#6AB3A2', '#4C8E80'), 2.5)}</g>`;
      });
      return svg(s);
    },

    ai() {
      const slabShape = rrect(-82, -82, 164, 164, 34);
      let s = shadow(slabShape);
      s += prism(slabShape, 0, 22, M.ceramic, 34);
      s += onTop(
        22,
        `<path d="${roundPath(-62, 26, 124, 30, 15)}" fill="#FCFAF5" stroke="rgba(40,34,30,.12)"/>
         <text x="-50" y="45" font-family="Geist, sans-serif" font-size="11" fill="rgba(40,34,30,.6)">72°F in °C?</text>
         <circle cx="46" cy="41" r="9" fill="${LILAC}"/>
         <path d="M42 41h8M47 37.5l3.5 3.5-3.5 3.5" stroke="#fff" stroke-width="1.6" fill="none" stroke-linecap="round"/>`
      );
      const shadowC = P(-4, -4, 22);
      s += `<ellipse cx="${f(shadowC[0])}" cy="${f(shadowC[1])}" rx="34" ry="17" fill="rgba(60,40,100,.28)" filter="url(#isoSoft)" class="halo"/>`;
      const star = (sz) =>
        `<path d="M0 ${-sz} C${sz * 0.12} ${-sz * 0.3} ${sz * 0.3} ${-sz * 0.12} ${sz} 0 C${sz * 0.3} ${sz * 0.12} ${sz * 0.12} ${sz * 0.3} 0 ${sz} C${-sz * 0.12} ${sz * 0.3} ${-sz * 0.3} ${sz * 0.12} ${-sz} 0 C${-sz * 0.3} ${-sz * 0.12} ${-sz * 0.12} ${-sz * 0.3} 0 ${-sz}Z" fill="__FILL__"/>`;
      const big = `<g transform="translate(0 40)">${star(36)}</g>`;
      const small = `<g transform="translate(46 4)">${star(14)}</g>`;
      s += `<g class="float">${slab(0, -4, 130, big + small, '#6E5FC0', 6)}${onLeft(0, -4, 130, (big + small).replace(/__FILL__/g, LILAC))}</g>`;
      return svg(s);
    },

    privacy() {
      let s = shadow(rrect(-46, -34, 92, 68, 12));
      s += box(-46, -34, 92, 68, 0, 10, M.stone, 12);
      s += box(-7, -7, 14, 14, 10, 44, M.graphite, 4);
      const body = rrect(-76, -26, 152, 52, 26);
      s += prism(body, 44, 94, M.ceramic, 26);
      s += onLeft(
        26,
        -50,
        94,
        `<circle cx="50" cy="25" r="19" fill="#0b0b0b"/>
         <circle cx="50" cy="25" r="13" fill="#1d2230"/>
         <circle cx="50" cy="25" r="7" fill="#2d3c5a"/>
         <ellipse cx="45" cy="20" rx="4" ry="2.6" fill="rgba(255,255,255,.55)"/>
         <circle class="led led-g" cx="14" cy="25" r="4" fill="${GREEN}"/>
         <circle class="led led-o" cx="86" cy="25" r="4" fill="${ORANGE}"/>`
      );
      return svg(s);
    },

    ports() {
      const strip = rrect(-104, -44, 208, 88, 16);
      let s = shadow(strip);
      s += prism(strip, 0, 24, M.ceramic, 16);
      const labels = [':3000', ':5173', ':8080'];
      let top = '';
      [-88, -22, 44].forEach((x, i) => {
        top += `<path d="${roundPath(x, -30, 44, 46, 10)}" fill="#2A2521"/>`;
        top += `<rect x="${x + 13}" y="-16" width="4" height="12" rx="2" fill="#0d0c0b"/><rect x="${x + 27}" y="-16" width="4" height="12" rx="2" fill="#0d0c0b"/>`;
        top += `<text x="${x + 4}" y="32" font-family="Geist Mono, monospace" font-size="10" font-weight="500" fill="rgba(40,34,30,.55)">${labels[i]}</text>`;
      });
      s += onTop(24, top);
      s += onLeft(
        44,
        -104,
        24,
        [38, 104, 170].map((u, i) => `<circle class="led led-${i}" cx="${u}" cy="12" r="3" fill="${GREEN}"/>`).join('')
      );
      s += box(-14, -21, 28, 28, 24, 54, M.graphite, 6);
      const c0 = P(0, -7, 54);
      s += `<path d="M${f(c0[0])} ${f(c0[1])} C${f(c0[0])} ${f(c0[1] - 40)} ${f(c0[0] + 60)} ${f(c0[1] - 60)} ${f(c0[0] + 100)} ${f(c0[1] - 52)}" fill="none" stroke="#2D2824" stroke-width="8" stroke-linecap="round"/>`;
      return svg(s);
    },

    locks() {
      const plate = rrect(-112, -52, 224, 104, 14);
      let s = shadow(plate);
      s += prism(plate, 0, 12, M.graphite, 14);
      const keys = [
        { x: -98, w: 98, label: 'Caps Lock', led: true },
        { x: 8, w: 42, label: 'Num' },
        { x: 58, w: 42, label: 'Scroll' },
      ];
      keys.forEach((k, i) => {
        const shape = rrect(k.x, -38, k.w, 76, 10);
        let art = `<text x="${k.x + 12}" y="-14" font-family="Geist, sans-serif" font-size="11" font-weight="500" fill="rgba(40,34,30,.6)">${k.label}</text>`;
        if (k.led) art += `<circle class="led-key" cx="${k.x + 16}" cy="18" r="4.5" fill="#B9AE9B"/>`;
        s += `<g class="key key-${i}">${prism(shape, 12, 32, M.ceramic, 10)}${onTop(32, art)}</g>`;
      });
      return svg(s);
    },

    battery() {
      const body = rrect(-94, -48, 176, 96, 20);
      let s = shadow(body);
      s += box(80, -18, 18, 36, 8, 24, M.stone, 6);
      s += prism(body, 0, 32, M.ceramic, 20);
      s += onTop(
        32,
        `<path d="${roundPath(-80, -34, 148, 68, 11)}" fill="#2A2521"/>
         <rect class="grow-x" x="-74" y="-28" width="136" height="56" rx="7" fill="${GREEN}"/>
         <path d="M-2 -20 -14 3h10l-4 17 16-24H-2z" fill="#FCFAF5"/>`
      );
      s += onLeft(48, -94, 32, `<text x="16" y="21" font-family="Geist, sans-serif" font-size="13" font-weight="600" fill="rgba(40,34,30,.55)">86%</text>`);
      return svg(s);
    },

    claude() {
      const term = rrect(-98, -68, 196, 136, 14);
      let s = shadow(term, 0.6);
      s += prism(term, 0, 14, M.black, 14);
      s += onTop(
        14,
        `<circle cx="-82" cy="-54" r="3.5" fill="#3a3632"/><circle cx="-71" cy="-54" r="3.5" fill="#3a3632"/><circle cx="-60" cy="-54" r="3.5" fill="#3a3632"/>
         <text x="-82" y="-24" font-family="Geist Mono, monospace" font-size="10" fill="#EFE8DC" opacity=".9">&gt; fix the peek queue</text>
         <text x="-82" y="-6" font-family="Geist Mono, monospace" font-size="10" fill="#A39A8D">● Edit src/app.rs</text>
         <text x="-82" y="12" font-family="Geist Mono, monospace" font-size="10" fill="#A39A8D">● cargo test</text>
         <text x="-82" y="30" font-family="Geist Mono, monospace" font-size="10" fill="${GREEN}">✓ Done in 1m 12s</text>
         <rect x="-82" y="40" width="7" height="12" fill="#EFE8DC" class="blink"/>`
      );
      const pill = `<path d="${roundPath(0, 0, 150, 42, 21)}" fill="__FILL__"/>`;
      const content =
        `<circle cx="22" cy="21" r="12" fill="${CLAUDE}" opacity=".25"/>` +
        `<path d="M22 13c.4 3.3 2.4 5.3 5.6 5.6-3.2.4-5.2 2.4-5.6 5.6-.4-3.2-2.4-5.2-5.6-5.6 3.2-.3 5.2-2.3 5.6-5.6z" fill="${CLAUDE}"/>` +
        `<text x="42" y="19" font-family="Geist, sans-serif" font-size="11" font-weight="600" fill="#fff">Claude finished</text>` +
        `<text x="42" y="32" font-family="Geist, sans-serif" font-size="9.5" fill="rgba(255,255,255,.55)">Click to open terminal</text>`;
      s += `<g class="float">${slab(-20, -84, 104, pill, '#141210', 5)}${onLeft(-20, -84, 104, pill.replace('__FILL__', '#000') + content)}</g>`;
      return svg(s);
    },
  };

  function roundPath(x, y, w, h, r) {
    r = Math.min(r, w / 2, h / 2);
    return `M${x + r} ${y}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1 ${-r} ${r}h${-(w - 2 * r)}a${r} ${r} 0 0 1 ${-r} ${-r}v${-(h - 2 * r)}a${r} ${r} 0 0 1 ${r} ${-r}z`;
  }

  // ---------------------------------------------------------------- laptop

  /** White laptop with its screen facing front-left. Returns the SVG and the
      CSS matrix that maps a W x H HTML element onto the screen. */
  function laptop() {
    const DW = 440;
    const DD = 300;
    const DT = 14;
    const LID_H = 286;
    const x0 = -DW / 2;
    const y0 = -DD / 2;
    let s = shadow(rrect(x0, y0, DW, DD, 20), 0.6, 14, 12);
    s += prism(rrect(x0, y0, DW, DD, 20), 0, DT, M.ceramic, 20);
    // keyboard + trackpad
    let keys = '';
    const kw = 24;
    const gap = 5;
    const cols = 14;
    const rows = 5;
    const kx0 = -((cols * kw + (cols - 1) * gap) / 2);
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        let w = kw;
        let x = kx0 + c * (kw + gap);
        if (r === 4 && c >= 4 && c <= 9) {
          if (c > 4) continue;
          w = kw * 6 + gap * 5;
        }
        keys += `<rect x="${f(x)}" y="${-112 + r * (kw + gap)}" width="${f(w)}" height="${kw}" rx="5" fill="#E3DBCD" stroke="rgba(0,0,0,.06)"/>`;
      }
    }
    s += onTop(
      DT,
      `<path d="${roundPath(kx0 - 10, -122, -2 * kx0 + 20, rows * (kw + gap) + 15, 12)}" fill="#D2C9B9"/>${keys}
       <path d="${roundPath(-78, 54, 156, 78, 10)}" fill="#E4DCCE" stroke="rgba(0,0,0,.07)"/>`
    );
    // lid
    const LT = 10;
    s += prism(rrect(x0, y0, DW, LT, 5), DT, DT + LID_H, M.ceramic, 5);
    const sy = y0 + LT;
    // bezel + screen
    s += onLeft(sy, x0, DT + LID_H, `<path d="${roundPath(8, 8, DW - 16, LID_H - 22, 10)}" fill="#0b0b0b"/>`);
    // screen element: 412 x 248 starting at (14, 14) on the face
    const SW = DW - 28;
    const SH = LID_H - 34;
    const o = P(x0 + 14, sy, DT + LID_H - 14);
    const matrix = `matrix(${f(C)},${S},0,1,${f(o[0])},${f(o[1])})`;
    return { svg: svg(s, '-336 -505 672 705', 'iso-laptop'), matrix, o, C, S, w: SW, h: SH, vb: { x: -336, y: -505, w: 672, h: 705 } };
  }

  // ------------------------------------------------------- exploded layers

  /** Four stacked composition layers. Each layer is a <g class="layer">
      that the page moves apart along z (screen y). */
  function layers() {
    const out = [];
    const PW = 300;
    const PD = 96;
    // desktop
    {
      const shape = rrect(-210, -140, 420, 280, 14);
      let s = prism(shape, 0, 4, { top: '#3B2E3A', l: '#2A2229', r: '#1C171C' }, 14);
      s += onTop(
        4,
        `<path d="${roundPath(-210, -140, 420, 280, 14)}" fill="url(#isoWall)"/>
         <path d="${roundPath(-170, -40, 220, 150, 8)}" fill="#24201C" opacity=".92"/>
         <rect x="-170" y="-40" width="220" height="18" rx="8" fill="#2E2925"/>
         ${[0, 1, 2, 3, 4, 5].map((i) => `<rect x="-154" y="${-6 + i * 16}" width="${[120, 160, 90, 140, 70, 110][i]}" height="6" rx="3" fill="${['#C792EA', '#82AAFF', '#C3E88D', '#F78C6C', '#89DDFF', '#A39A8D'][i]}" opacity=".75"/>`).join('')}
         <path d="${roundPath(-30, -110, 190, 120, 8)}" fill="#EFE8DC" opacity=".85"/>
         <rect x="-14" y="-92" width="90" height="8" rx="4" fill="#A897F5"/>
         <rect x="-14" y="-74" width="150" height="6" rx="3" fill="rgba(40,34,30,.25)"/>
         <rect x="-14" y="-62" width="120" height="6" rx="3" fill="rgba(40,34,30,.25)"/>`
      );
      out.push({ s, label: 'Your desktop', api: 'whatever is behind the notch', anchor: P(210, -140, 4) });
    }
    // backdrop blur
    {
      const shape = rrect(-PW / 2, -PD / 2, PW, PD, 30);
      let s = prism(shape, 0, 3, { top: 'rgba(239,232,220,.16)', l: 'rgba(239,232,220,.22)', r: 'rgba(239,232,220,.12)', edge: 'rgba(255,255,255,.35)' }, 30);
      s += onTop(3, `<path d="${roundPath(-PW / 2, -PD / 2, PW, PD, 30)}" fill="url(#isoFrost)" opacity=".9"/>`);
      out.push({ s, label: 'Backdrop blur', api: 'HostBackdropBrush', anchor: P(PW / 2, -PD / 2, 3) });
    }
    // content
    {
      const shape = rrect(-PW / 2, -PD / 2, PW, PD, 30);
      let s = prism(shape, 0, 3, { top: 'rgba(0,0,0,.82)', l: '#151311', r: '#080706', edge: 'rgba(255,255,255,.2)' }, 30);
      s += onTop(
        3,
        `<path d="${roundPath(-136, -26, 128, 62, 10)}" fill="rgba(255,255,255,.08)"/>
         <rect x="-128" y="-18" width="30" height="30" rx="7" fill="${SEA}"/>
         <rect x="-92" y="-14" width="56" height="6" rx="3" fill="rgba(255,255,255,.85)"/>
         <rect x="-92" y="-3" width="40" height="5" rx="2.5" fill="rgba(255,255,255,.45)"/>
         <rect x="-128" y="22" width="112" height="4" rx="2" fill="rgba(255,255,255,.18)"/>
         <rect x="-128" y="22" width="44" height="4" rx="2" fill="${SEA}"/>
         ${[0, 1, 2].map((i) => `<path d="${roundPath(0, -26 + i * 22, 136, 18, 6)}" fill="rgba(255,255,255,.08)"/><circle cx="12" cy="${-17 + i * 22}" r="5" fill="${[HONEY, '#EFE8DC', BLUE][i]}" opacity=".9"/><rect x="24" y="${-19 + i * 22}" width="${[50, 70, 44][i]}" height="4" rx="2" fill="rgba(255,255,255,.6)"/>`).join('')}
         ${[-128, -114, -100, -86, -72, -58, -44].map((x) => `<circle cx="${x}" cy="-38" r="4" fill="rgba(255,255,255,.3)"/>`).join('')}`
      );
      out.push({ s, label: 'Content', api: 'Direct2D, composition swap chain', anchor: P(PW / 2, -PD / 2, 3) });
    }
    // bars
    {
      let s = onTop(0, `<path d="${roundPath(-PW / 2, -PD / 2, PW, PD, 30)}" fill="none" stroke="rgba(239,232,220,.28)" stroke-dasharray="4 5"/>`);
      [0, 1, 2, 3].forEach((i) => {
        s += box(-44 + i * 7, -16, 4, 4, 0, [10, 18, 7, 14][i], tint(SEA, '#6AB3A2', '#4C8E80'), 2);
      });
      out.push({ s, label: 'Visualizer', api: 'animated by the compositor', anchor: P(PW / 2, -PD / 2, 0) });
    }
    return out;
  }

  /** Shared <defs> the objects reference by id. */
  const DEFS = `
    <filter id="isoSoft" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="7"/></filter>
    <radialGradient id="isoLabel" cx=".35" cy=".35" r=".7"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient>
    <pattern id="isoMesh" width="4" height="4" patternUnits="userSpaceOnUse"><circle cx="2" cy="2" r=".8" fill="rgba(255,255,255,.22)"/></pattern>
    <linearGradient id="isoWall" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3D2B45"/><stop offset=".55" stop-color="#9A4E4E"/><stop offset="1" stop-color="#E9A35A"/></linearGradient>
    <linearGradient id="isoFrost" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="rgba(255,255,255,.28)"/><stop offset="1" stop-color="rgba(255,255,255,.04)"/></linearGradient>`;

  window.ISO = { objects, laptop, layers, P, DEFS };
})();
