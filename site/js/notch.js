/* A web replica of the Dynamic Notch: same geometry, same springs, same
   state machine (idle -> peek -> expanded) as src/app.rs and src/anim.rs.
   Frames are only drawn while a spring is moving, like src/pacer.rs. */
(function () {
  'use strict';

  const REDUCED = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const SUBSTEP = 1 / 480;

  // Defaults from src/config.rs and the layout constants in src/app.rs.
  const BASE_W = 188;
  const BASE_H = 32;
  const EXP_W = 600;
  const HEADER_H = 42;
  const BOTTOM_PAD = 18;
  const HOME_H = 128;
  const BOUNCE = 0.3;

  // ---------------------------------------------------------------- springs

  class Spring {
    constructor(value, response, damping) {
      this.value = value;
      this.target = value;
      this.velocity = 0;
      this.eps = 0.01;
      this.set(response, damping);
    }
    set(response, damping) {
      const w = (Math.PI * 2) / Math.max(response, 0.02);
      this.k = w * w;
      this.c = 2 * damping * w;
    }
    to(t) {
      this.target = t;
    }
    snap(v) {
      this.value = this.target = v;
      this.velocity = 0;
    }
    get moving() {
      return this.value !== this.target || this.velocity !== 0;
    }
    step(dt) {
      if (!this.moving) return false;
      if (REDUCED) {
        this.snap(this.target);
        return false;
      }
      let t = dt;
      while (t > 0) {
        const h = Math.min(SUBSTEP, t);
        const a = -this.k * (this.value - this.target) - this.c * this.velocity;
        this.velocity += a * h;
        this.value += this.velocity * h;
        t -= h;
      }
      if (Math.abs(this.value - this.target) < this.eps && Math.abs(this.velocity) < this.eps * 10) {
        this.snap(this.target);
        return false;
      }
      return true;
    }
  }

  // ------------------------------------------------------------------ pacer

  const Pacer = {
    engines: new Set(),
    running: false,
    last: 0,
    frames: 0,
    wake(engine) {
      this.engines.add(engine);
      if (this.running) return;
      this.running = true;
      this.last = performance.now();
      requestAnimationFrame(this.tick);
    },
    tick(now) {
      const dt = Math.min(0.05, (now - Pacer.last) / 1000);
      Pacer.last = now;
      Pacer.frames++;
      for (const e of Pacer.engines) {
        if (!e.frame(dt)) Pacer.engines.delete(e);
      }
      if (Pacer.engines.size) requestAnimationFrame(Pacer.tick);
      else Pacer.running = false;
    },
  };

  // ------------------------------------------------------------------ icons

  const I = {
    play: '<path d="M8 5.5v13l10.5-6.5z" class="f"/>',
    pause: '<rect x="6.5" y="5" width="4" height="14" rx="1.3" class="f"/><rect x="13.5" y="5" width="4" height="14" rx="1.3" class="f"/>',
    next: '<path d="M5 6v12l9-6z" class="f"/><rect x="15.5" y="6" width="2.6" height="12" rx="1.1" class="f"/>',
    prev: '<path d="M19 6v12l-9-6z" class="f"/><rect x="5.9" y="6" width="2.6" height="12" rx="1.1" class="f"/>',
    music: '<path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/>',
    timer: '<circle cx="12" cy="13.5" r="7.5"/><path d="M12 13.5V9.5M9.5 2.8h5M18.6 6.4l1.4-1.4"/>',
    clipboard: '<rect x="5.5" y="4.5" width="13" height="16.5" rx="2.5"/><path d="M9 3.5h6v3H9zM9 11.5h6M9 15h4"/>',
    download: '<path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19.5h14"/>',
    mic: '<rect x="9" y="3.5" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v2.5"/>',
    camera: '<rect x="3" y="7" width="13" height="10" rx="2.5"/><path d="m16 10.5 5-3v9l-5-3z"/>',
    sparkle: '<path d="M12 3c.6 4.6 3.4 7.4 8 8-4.6.6-7.4 3.4-8 8-.6-4.6-3.4-7.4-8-8 4.6-.6 7.4-3.4 8-8z" class="f"/>',
    check: '<path d="m5.5 12.5 4 4 9-9.5"/>',
    keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2.5"/><path d="M6.5 10h.01M10 10h.01M13.5 10h.01M17 10h.01M8 14.5h8"/>',
    folder: '<path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>',
    home: '<path d="M4.5 10.5 12 4.5l7.5 6v8a1.5 1.5 0 0 1-1.5 1.5h-3.5v-5h-5v5H6a1.5 1.5 0 0 1-1.5-1.5z"/>',
    file: '<path d="M6.5 3.5h7l4 4v12a1 1 0 0 1-1 1h-10a1 1 0 0 1-1-1v-15a1 1 0 0 1 1-1z"/><path d="M13.5 3.5v4h4"/>',
    terminal: '<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><path d="m7 9.5 3 2.5-3 2.5M12.5 15h4.5"/>',
    volume: '<path d="M4.5 9.5h3l4.5-4v13l-4.5-4h-3z"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>',
    plug: '<path d="M9 3.5v4M15 3.5v4M6.5 7.5h11v3a5.5 5.5 0 0 1-11 0zM12 16v4.5"/>',
    battery: '<rect x="2.5" y="7" width="17" height="10" rx="2.5"/><path d="M21.5 10.5v3"/>',
    bolt: '<path d="M13 3 5.5 13.5H12l-1 7.5 7.5-10.5H12z" class="f"/>',
    tray: '<path d="M3.5 13.5 6 6.5a1.5 1.5 0 0 1 1.4-1h9.2a1.5 1.5 0 0 1 1.4 1l2.5 7v4.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3.5 18z"/><path d="M3.5 13.5h5l1 2h5l1-2h5"/>',
    lock: '<rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>',
    bell: '<path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15zM10 20.5h4"/>',
    globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.5 2.4 3.5 5.2 3.5 8.5s-1 6.1-3.5 8.5c-2.5-2.4-3.5-5.2-3.5-8.5s1-6.1 3.5-8.5z"/>',
  };
  const icon = (name, cls = '') => `<svg class="ic ${cls}" viewBox="0 0 24 24" aria-hidden="true">${I[name] || ''}</svg>`;

  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  const alpha = (hex, a) => {
    const n = parseInt(hex.slice(1), 16);
    return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
  };

  // ------------------------------------------------------------------ slots

  function ring(p, color, size = 18, width = 2.6) {
    const r = (size - width) / 2;
    const c = 2 * Math.PI * r;
    const dash = p == null ? `${c * 0.28} ${c}` : `${c * Math.max(0, Math.min(1, p))} ${c}`;
    return `<svg class="ring${p == null ? ' spin' : ''}" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}" aria-hidden="true">
      <circle cx="${size / 2}" cy="${size / 2}" r="${r}" fill="none" stroke="${alpha(color, 0.22)}" stroke-width="${width}"/>
      <circle cx="${size / 2}" cy="${size / 2}" r="${r}" fill="none" stroke="${color}" stroke-width="${width}" stroke-linecap="round"
        stroke-dasharray="${dash}" transform="rotate(-90 ${size / 2} ${size / 2})"/></svg>`;
  }

  const bars = (color, playing = true) =>
    `<span class="bars${playing ? '' : ' paused'}" style="color:${color}"><i></i><i></i><i></i><i></i></span>`;

  function battery(level, charging, color) {
    const c = color || (level < 0.2 ? '#FF453A' : '#34C759');
    return `<span class="batt"><span class="batt-body"><span class="batt-fill" style="width:${Math.round(level * 100)}%;background:${c}"></span></span>${
      charging ? icon('bolt', 'batt-bolt') : ''
    }</span>`;
  }

  function slot(s) {
    if (!s) return '';
    switch (s.type) {
      case 'art':
        return `<span class="art" style="background:${s.art}"></span>`;
      case 'icon':
        return `<span class="slot-icon" style="color:${s.color}">${icon(s.icon)}</span>`;
      case 'text':
        return `<span class="slot-text" style="color:${s.color || '#fff'}">${esc(s.text)}</span>`;
      case 'bars':
        return bars(s.color, s.playing !== false);
      case 'ring':
        return ring(s.p, s.color);
      case 'battery':
        return battery(s.level, s.charging);
      case 'dots':
        return `<span class="dots">${s.colors.map((c) => `<i style="background:${c}"></i>`).join('')}</span>`;
      case 'wave':
        return `<span class="wave" style="color:${s.color}">${'<i></i>'.repeat(9)}</span>`;
      default:
        return '';
    }
  }

  // ------------------------------------------------------------------ panel

  function panelHTML(d) {
    const m = d.media;
    const t = d.timer;
    const tabs = ['home', 'music', 'timer', 'clipboard', 'download', 'tray', 'sparkle']
      .map((n, i) => `<span class="tab${i === 0 ? ' on' : ''}">${icon(n)}</span>`)
      .join('');
    return `
      <div class="p-head">
        <div class="p-tabs">${tabs}</div>
        <div class="p-ind">${d.mic ? '<i class="p-dot" style="background:#34C759"></i>' : ''}${battery(0.86, true)}<span>86%</span></div>
      </div>
      <div class="p-home">
        <div class="card card-media">
          <div class="cm-top">
            <span class="art art-lg" style="background:${m.art}"></span>
            <div class="cm-meta"><b>${esc(m.title)}</b><span>${esc(m.artist)}</span></div>
            ${bars(m.accent, m.playing)}
          </div>
          <div class="cm-bottom">
            <span class="btn-ic">${icon('prev')}</span>
            <span class="btn-ic btn-play">${icon(m.playing ? 'pause' : 'play')}</span>
            <span class="btn-ic">${icon('next')}</span>
            <span class="scrub"><i style="width:${Math.round(m.progress * 100)}%;background:${m.accent}"></i></span>
          </div>
        </div>
        <div class="p-col">
          <div class="card row">
            <span class="row-ic" style="color:#E9B44C;background:${alpha('#E9B44C', 0.16)}">${icon('timer')}</span>
            <div class="row-meta"><b>Timer</b><span class="tnum">${esc(t.label)}</span></div>
            <span class="pill${t.pressed ? ' pressed' : ''}" data-hit="timer">${t.running ? 'Pause' : 'Start'}</span>
          </div>
          <div class="card row">
            <span class="row-ic" style="color:#EFE8DC;background:rgba(255,255,255,.08)">${icon('clipboard')}</span>
            <div class="row-meta"><b class="mono">cargo build --release</b><span>Copied 2 min ago</span></div>
          </div>
          <div class="card row">
            <span class="row-ic" style="color:#5AA9F0;background:${alpha('#5AA9F0', 0.16)}">${icon('download')}</span>
            <div class="row-meta"><b>notch-0.2.0.zip</b><span class="prog"><i style="width:${d.download}%"></i></span></div>
            <span class="row-val tnum">${d.download}%</span>
          </div>
        </div>
      </div>`;
  }

  // ------------------------------------------------------------------- peek

  function peekHTML(p) {
    const lead = p.art
      ? `<span class="art art-peek" style="background:${p.art}"></span>`
      : `<span class="pk-ic" style="color:${p.accent};background:${alpha(p.accent, 0.18)}">${icon(p.icon)}</span>`;
    let trail = '';
    let level = '';
    const t = p.trailing;
    if (t) {
      if (t.type === 'check') trail = `<span class="pk-check">${icon('check')}</span>`;
      else if (t.type === 'text') trail = `<span class="pk-text" style="color:${t.color || p.accent}">${esc(t.text)}</span>`;
      else if (t.type === 'ring') trail = ring(t.p, p.accent, 24, 3);
      else if (t.type === 'battery') trail = battery(t.level, t.charging);
      else if (t.type === 'button') trail = `<span class="pk-btn" style="background:${alpha(p.accent, 0.2)};color:${p.accent}">${esc(t.label)}</span>`;
      else if (t.type === 'bars') trail = bars(p.accent, true);
      else if (t.type === 'wave') trail = slot({ type: 'wave', color: p.accent });
      else if (t.type === 'dots') trail = slot({ type: 'dots', colors: t.colors });
      else if (t.type === 'level')
        level = `<span class="pk-level"><span><i style="width:${Math.round(t.frac * 100)}%;background:${p.accent}"></i></span><em class="tnum">${Math.round(t.frac * 100)}%</em></span>`;
    }
    return `${lead}<div class="pk-meta"><b>${esc(p.title)}</b>${level || `<span>${esc(p.subtitle || '')}</span>`}</div>${trail}`;
  }

  // ------------------------------------------------------------------ notch

  const DEFAULT_PANEL = {
    media: { title: 'Glasshouse', artist: 'Ilse Moreau', art: '#333', accent: '#7FC8B5', playing: true, progress: 0.38 },
    timer: { label: '25:00', running: false, pressed: false },
    download: 42,
    mic: false,
  };

  class Notch {
    constructor(el, opts = {}) {
      this.el = el;
      this.opts = opts;
      this.scale = opts.scale || 1;
      el.classList.add('notch');
      el.innerHTML = `
        <div class="n-shadow"></div>
        <div class="n-body">
          <div class="n-layer n-act"><span class="n-slot n-left"></span><span class="n-dots"></span><span class="n-slot n-right"></span></div>
          <div class="n-layer n-peek"></div>
          <div class="n-layer n-peek"></div>
          <div class="n-layer n-panel"></div>
        </div>
        <div class="n-bubble"><span class="n-slot"></span></div>`;
      this.body = el.querySelector('.n-body');
      this.shadow = el.querySelector('.n-shadow');
      this.actEl = el.querySelector('.n-act');
      this.leftEl = el.querySelector('.n-left');
      this.rightEl = el.querySelector('.n-right');
      this.dotsEl = el.querySelector('.n-dots');
      this.peekEls = [...el.querySelectorAll('.n-peek')];
      this.panelEl = el.querySelector('.n-panel');
      this.bubble = el.querySelector('.n-bubble');
      this.bubbleSlot = this.bubble.querySelector('.n-slot');
      this.peekIdx = 0;

      const r0 = Math.min(BASE_H * 0.36, 13);
      const e0 = Math.max(4, Math.min(8, BASE_H * 0.22));
      this.a = {
        w: new Spring(BASE_W, 0.5, 0.82),
        h: new Spring(BASE_H, 0.5, 0.82),
        r: new Spring(r0, 0.5, 0.82),
        ear: new Spring(e0, 0.4, 1),
        glass: new Spring(0, 0.24, 1),
        shown: new Spring(opts.hidden ? 0 : 1, 0.5, 1 - 0.5 * BOUNCE),
        sec: new Spring(0, 0.45, 1 - 0.6 * BOUNCE),
      };
      this.a.glass.eps = this.a.shown.eps = this.a.sec.eps = 0.002;

      this.mode = 'idle';
      this.activity = null;
      this.secondary = null;
      this.indicators = [];
      this.hovering = false;
      this.expandedBy = null;
      this.peekQueue = [];
      this.current = null;
      this.peekTimer = 0;
      this.hoverTimer = 0;
      this.shownFlag = !opts.hidden;
      this.panel = JSON.parse(JSON.stringify(DEFAULT_PANEL));
      this.onState = opts.onState || null;

      if (opts.interactive) this.bindHover();
      this.renderPanel();
      this.syncContent();
      this.retarget();
      this.frame(0);
    }

    // ---- public API -------------------------------------------------------

    setActivity(a) {
      this.activity = a;
      if (a) {
        this.leftEl.innerHTML = slot(a.left);
        this.rightEl.innerHTML = slot(a.right);
      }
      this.syncContent();
      this.retarget();
    }
    setSecondary(s) {
      this.secondary = s;
      if (s) this.bubbleSlot.innerHTML = slot(s);
      this.retarget();
    }
    setIndicators(colors) {
      this.indicators = colors || [];
      this.dotsEl.innerHTML = this.indicators.map((c) => `<i style="background:${c}"></i>`).join('');
      this.retarget();
    }
    setPanel(patch) {
      for (const k of Object.keys(patch)) {
        const v = patch[k];
        this.panel[k] = v && typeof v === 'object' && !Array.isArray(v) ? Object.assign({}, this.panel[k], v) : v;
      }
      this.renderPanel();
    }
    setShown(on) {
      if (this.shownFlag === on) return;
      this.shownFlag = on;
      this.el.classList.toggle('is-hidden', !on);
      this.retarget();
    }

    /** Show a transient banner. p: {icon, accent, art, title, subtitle, trailing, duration, key, instant} */
    peek(p) {
      p = Object.assign({ accent: '#EFE8DC', icon: 'bell', duration: 3200 }, p);
      if (p.key) this.peekQueue = this.peekQueue.filter((q) => q.key !== p.key);
      if (this.current && p.key && this.current.key === p.key) {
        // same key: update the visible banner in place
        const el = this.peekEls[this.peekIdx];
        const lv = el.querySelector('.pk-level');
        if (lv && p.trailing && p.trailing.type === 'level') {
          lv.querySelector('i').style.width = Math.round(p.trailing.frac * 100) + '%';
          lv.querySelector('em').textContent = Math.round(p.trailing.frac * 100) + '%';
        } else el.innerHTML = peekHTML(p);
        this.current = p;
        this.armPeekTimer(p);
        return;
      }
      if (this.mode === 'peek' && !p.instant) {
        this.peekQueue.push(p);
        return;
      }
      if (this.mode === 'expanded') {
        if (!p.instant) {
          this.peekQueue.push(p);
          return;
        }
        this.expandedBy = null;
      }
      this.showPeek(p);
    }
    clearPeeks() {
      this.peekQueue = [];
      clearTimeout(this.peekTimer);
      if (this.mode === 'peek') {
        this.current = null;
        this.mode = 'idle';
        this.syncContent();
        this.retarget();
      }
    }
    expand(by = 'api') {
      clearTimeout(this.peekTimer);
      if (this.current) {
        this.current = null;
      }
      this.mode = 'expanded';
      this.expandedBy = by;
      this.syncContent();
      this.retarget();
    }
    collapse() {
      if (this.mode !== 'expanded') return;
      this.mode = 'idle';
      this.expandedBy = null;
      this.syncContent();
      this.retarget();
      if (this.peekQueue.length) setTimeout(() => this.nextPeek(), 260);
    }
    toggle() {
      if (this.mode === 'expanded') this.collapse();
      else this.expand('click');
    }
    hover(on) {
      if (this.hovering === on) return;
      this.hovering = on;
      clearTimeout(this.hoverTimer);
      if (on) {
        // hover_delay_ms = 140
        this.hoverTimer = setTimeout(() => {
          if (this.hovering && this.mode !== 'expanded') this.expand('hover');
        }, 140);
      } else if (this.mode === 'expanded' && this.expandedBy === 'hover') {
        // collapse_delay_ms = 60
        this.hoverTimer = setTimeout(() => this.collapse(), 60);
      }
      this.retarget();
    }
    /** The hover lean without the hover-to-expand. */
    lean(on) {
      clearTimeout(this.hoverTimer);
      this.hovering = on;
      this.retarget();
    }
    /** Hit-test a panel element by data-hit name; returns its rect in page px. */
    hitRect(name) {
      const el = this.panelEl.querySelector(`[data-hit="${name}"]`);
      return el ? el.getBoundingClientRect() : null;
    }

    // ---- internals --------------------------------------------------------

    bindHover() {
      this.body.addEventListener('mouseenter', () => this.hover(true));
      this.body.addEventListener('mouseleave', () => this.hover(false));
      this.body.addEventListener('click', (e) => {
        if (this.mode === 'peek' && this.current && this.current.href) {
          location.href = this.current.href;
        } else if (this.mode === 'peek') {
          this.clearPeeks();
          this.expand('click');
        } else if (this.mode !== 'expanded' || this.expandedBy !== 'hover') {
          this.toggle();
        }
        e.stopPropagation();
      });
      this.body.setAttribute('tabindex', '0');
      this.body.setAttribute('role', 'button');
      this.body.setAttribute('aria-label', 'Dynamic Notch demo. Press Enter to open or close the panel.');
      this.body.addEventListener('focus', () => this.el.classList.toggle('is-focus', this.body.matches(':focus-visible')));
      this.body.addEventListener('blur', () => this.el.classList.remove('is-focus'));
      this.body.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          this.toggle();
        } else if (e.key === 'Escape') this.collapse();
      });
      document.addEventListener('click', (e) => {
        if (this.mode === 'expanded' && !this.el.contains(e.target)) this.collapse();
      });
    }

    showPeek(p) {
      this.current = p;
      this.mode = 'peek';
      this.peekIdx = 1 - this.peekIdx;
      this.peekEls[this.peekIdx].innerHTML = peekHTML(p);
      this.armPeekTimer(p);
      this.syncContent();
      this.retarget();
    }
    armPeekTimer(p) {
      clearTimeout(this.peekTimer);
      if (p.duration === Infinity) return;
      this.peekTimer = setTimeout(() => this.nextPeek(), p.duration);
    }
    nextPeek() {
      if (this.mode === 'expanded') return;
      const p = this.peekQueue.shift();
      if (p) this.showPeek(p);
      else {
        this.current = null;
        if (this.mode === 'peek') this.mode = 'idle';
        this.syncContent();
        this.retarget();
      }
    }

    renderPanel() {
      this.panelEl.innerHTML = panelHTML(this.panel);
      this.panelEl.style.height = HEADER_H + HOME_H + BOTTOM_PAD + 'px';
    }

    syncContent() {
      const m = this.mode;
      this.body.dataset.mode = m;
      this.actEl.classList.toggle('on', m === 'idle' && !!this.activity);
      this.peekEls.forEach((el, i) => el.classList.toggle('on', m === 'peek' && i === this.peekIdx));
      this.panelEl.classList.toggle('on', m === 'expanded');
      this.dotsEl.classList.toggle('on', m === 'idle' && this.indicators.length > 0);
      if (this.onState) this.onState(m, this);
    }

    targets() {
      const t = {
        w: BASE_W,
        h: BASE_H,
        r: Math.min(BASE_H * 0.36, 13),
        ear: Math.max(4, Math.min(8, BASE_H * 0.22)),
        glass: 0,
        sec: false,
      };
      if (this.mode === 'expanded') {
        Object.assign(t, { w: EXP_W, h: HEADER_H + HOME_H + BOTTOM_PAD, r: 34, ear: 12, glass: 1 });
      } else if (this.mode === 'peek') {
        Object.assign(t, { w: Math.min(Math.max(BASE_W + 150, 360), EXP_W), h: BASE_H + 46, r: 26, ear: 10, glass: 0.55 });
      } else {
        if (this.activity) {
          t.w = BASE_W + 2 * (this.activity.wing || 1) * BASE_H;
          t.sec = !!this.secondary;
        } else if (this.indicators.length) {
          t.w = BASE_W + 28;
        }
        if (this.hovering) {
          t.w += 14;
          t.h += 4;
          t.r += 1.5;
        }
      }
      return t;
    }

    retarget() {
      const t = this.targets();
      const a = this.a;
      const grow = t.w > a.w.value + 0.5 || t.h > a.h.value + 0.5;
      const [resp, damp] = grow ? [0.5, 1 - 0.6 * BOUNCE] : [0.3, 1 - 0.15 * BOUNCE];
      for (const s of [a.w, a.h, a.r]) s.set(resp, damp);
      a.w.to(t.w);
      a.h.to(t.h);
      a.r.to(t.r);
      a.ear.to(t.ear);
      a.glass.to(t.glass);
      a.shown.to(this.shownFlag ? 1 : 0);
      a.sec.to(t.sec && this.shownFlag ? 1 : 0);
      Pacer.wake(this);
    }

    frame(dt) {
      let moving = false;
      for (const k in this.a) if (this.a[k].step(dt)) moving = true;
      this.draw();
      return moving;
    }

    draw() {
      const a = this.a;
      const w = Math.max(1, a.w.value);
      const h = Math.max(1, a.h.value);
      const e = Math.max(0, Math.min(a.ear.value, h * 0.5));
      const r = Math.max(0, Math.min(a.r.value, w * 0.5, h - e));
      const g = a.glass.value;
      const shown = a.shown.value;
      const W = w + 2 * e;

      const path =
        `M0 0A${e} ${e} 0 0 1 ${e} ${e}L${e} ${h - r}A${r} ${r} 0 0 0 ${e + r} ${h}` +
        `L${e + w - r} ${h}A${r} ${r} 0 0 0 ${e + w} ${h - r}L${e + w} ${e}A${e} ${e} 0 0 1 ${W} 0Z`;
      const lift = (1 - shown) * (h + 16);

      const bs = this.body.style;
      bs.width = W + 'px';
      bs.height = h + 'px';
      bs.left = -W / 2 + 'px';
      bs.clipPath = `path('${path}')`;
      bs.transform = `translateY(${-lift}px)`;
      bs.background = `rgba(0,0,0,${1 - 0.18 * g})`;
      bs.setProperty('--e', e + 'px');
      bs.setProperty('--h', h + 'px');

      const ss = this.shadow.style;
      ss.width = w + 'px';
      ss.height = h + 'px';
      ss.left = -w / 2 + 'px';
      ss.borderRadius = `0 0 ${r}px ${r}px`;
      ss.transform = `translateY(${-lift}px)`;
      ss.opacity = (0.35 + 0.65 * g) * shown;

      const sec = a.sec.value;
      const bb = this.bubble.style;
      const bx = w / 2 + e + 8;
      bb.left = bx + 'px';
      bb.opacity = Math.max(0, Math.min(1, sec));
      bb.transform = `translateY(${-lift}px) scale(${0.4 + 0.6 * Math.max(0, sec)})`;

      this.el.style.setProperty('--ns', this.scale);
      if (this.opts.onFrame) this.opts.onFrame(w, h);
    }
  }

  window.DN = { Notch, Spring, Pacer, icon, slot, alpha, REDUCED };
})();
