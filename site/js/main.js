(function () {
  'use strict';

  const { Notch, Pacer, REDUCED } = window.DN;
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];

  const SEA = '#7FC8B5';
  const HONEY = '#E9B44C';
  const LILAC = '#A897F5';
  const BLUE = '#5AA9F0';
  const GREEN = '#34C759';
  const ORANGE = '#FF9F0A';
  const CLAUDE = '#D97757';
  const PAPER = '#EFE8DC';

  // Album art is drawn with gradients; the accent comes from the art, as in the app.
  const TRACKS = [
    {
      title: 'Glasshouse',
      artist: 'Ilse Moreau',
      accent: SEA,
      art: 'radial-gradient(circle at 70% 30%, #e8f6ee 0 9%, transparent 10%), radial-gradient(120% 90% at 20% 100%, #2c6b60 0 40%, transparent 41%), linear-gradient(150deg, #9ad8c6, #4f9c8c 60%, #1f4a43)',
    },
    {
      title: 'Night Bus',
      artist: 'Tamsin Ode',
      accent: HONEY,
      art: 'radial-gradient(circle at 30% 64%, #f7d27a 0 13%, transparent 14%), linear-gradient(180deg, #1c2340 0 58%, #3a2b52 58% 70%, #e9b44c 70% 74%, #241a33 74%)',
    },
    {
      title: 'Saltwater',
      artist: 'The Hollow Pines',
      accent: LILAC,
      art: 'conic-gradient(from 210deg at 62% 42%, #c8bdfb, #5b4ba8, #f2c6e4, #8f7ff0, #c8bdfb)',
    },
  ];
  let trackIdx = 0;
  const track = () => TRACKS[trackIdx % TRACKS.length];

  const fmt = (s) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  const musicAct = (t) => ({ left: { type: 'art', art: t.art }, right: { type: 'bars', color: t.accent }, wing: 1 });
  const mediaPanel = (t, playing = true) => ({ title: t.title, artist: t.artist, art: t.art, accent: t.accent, playing });

  /** A cancellable set of timeouts. */
  class Script {
    constructor() {
      this.ids = [];
    }
    at(ms, fn) {
      this.ids.push(setTimeout(fn, ms));
    }
    clear() {
      this.ids.forEach(clearTimeout);
      this.ids = [];
    }
  }

  $('#isoDefs').innerHTML = window.ISO.DEFS;

  // ------------------------------------------------------------------ bar

  // The bar tucks away while you scroll down, so the top edge belongs to the notch.
  const bar = $('#bar');
  let lastY = scrollY;
  const onScrollBar = () => {
    const y = scrollY;
    bar.classList.toggle('is-scrolled', y > 8);
    if (y > lastY + 6 && y > 240 && !bar.contains(document.activeElement)) bar.classList.add('is-tucked');
    else if (y < lastY - 6 || y <= 240) bar.classList.remove('is-tucked');
    if (Math.abs(y - lastY) > 6) lastY = y;
  };
  addEventListener('scroll', onScrollBar, { passive: true });
  bar.addEventListener('focusin', () => bar.classList.remove('is-tucked'));
  onScrollBar();

  // ---------------------------------------------------------------- stage

  const viewport = $('#stageViewport');
  const screen = $('#screen');
  let stageScale = 1;
  new ResizeObserver(([e]) => {
    const w = e.contentRect.width;
    stageScale = w >= 900 ? w / 1200 : w / 640;
    viewport.style.setProperty('--s', stageScale);
  }).observe(viewport);

  // editor lines behind the glass, so the blur has something to blur
  {
    const palette = ['#C792EA', '#82AAFF', '#C3E88D', '#F78C6C', '#89DDFF', '#7d7486', '#FFCB6B'];
    const rows = [
      [[2, 40], [1, 70], [6, 30]],
      [[6, 20], [0, 60], [4, 110]],
      [[6, 40], [3, 90], [2, 50], [6, 40]],
      [[6, 60], [1, 120]],
      [[0, 50], [4, 70], [5, 140]],
      [[6, 40], [2, 80], [3, 60]],
      [[5, 200]],
      [[0, 70], [1, 60], [6, 90], [4, 40]],
      [[6, 30], [3, 110], [2, 40]],
      [[6, 60], [0, 50], [1, 80]],
      [[4, 90], [6, 60], [3, 70]],
      [[5, 160]],
      [[0, 40], [2, 100], [6, 50]],
      [[6, 50], [1, 70], [4, 90], [3, 40]],
      [[6, 80], [2, 60]],
      [[0, 60], [5, 120]],
      [[3, 70], [6, 40], [1, 90]],
      [[6, 30], [4, 80]],
      [[2, 50], [0, 60], [6, 120]],
      [[5, 90], [3, 60]],
      [[6, 40], [1, 110], [2, 50]],
      [[0, 70], [6, 90]],
    ];
    $('#editorCode').innerHTML = rows
      .map((r, i) => `<div class="ln" style="padding-left:${[0, 16, 16, 32, 32, 16, 0, 16, 32, 32, 16, 0, 16, 32, 32, 16, 0, 16, 32, 16, 16, 0][i]}px">${r.map(([c, w]) => `<i style="width:${w}px;background:${palette[c]}"></i>`).join('')}</div>`)
      .join('');
  }

  const sn = new Notch($('#stageNotch'), { hidden: true });
  const cursor = $('#cursor');
  const moveCursor = (x, y) => (cursor.style.transform = `translate(${x}px, ${y}px)`);
  const clickCursor = () => {
    cursor.classList.remove('click');
    void cursor.getBoundingClientRect();
    cursor.classList.add('click');
  };
  const termLines = $$('#stageTerm .t-late');

  const stage = { left: 25 * 60, running: false, iv: 0 };
  const timerAct = () => ({
    left: { type: 'ring', p: stage.left / 1500, color: HONEY },
    right: { type: 'text', text: fmt(stage.left), color: HONEY },
    wing: 1.55,
  });
  const musicBubble = () => ({ type: 'art', art: track().art });
  function startTimer() {
    if (stage.running) return;
    stage.running = true;
    clearInterval(stage.iv);
    stage.iv = setInterval(() => {
      stage.left = Math.max(0, stage.left - 1);
      if (sn.activity && sn.activity.right.type === 'text') sn.setActivity(timerAct());
      sn.setPanel({ timer: { label: fmt(stage.left) } });
    }, 1000);
  }
  function stopTimer() {
    stage.running = false;
    clearInterval(stage.iv);
    stage.left = 25 * 60;
  }

  /** Every scene starts from a known state, so the chips can jump anywhere. */
  function baseline(kind) {
    sn.clearPeeks();
    sn.hover(false);
    sn.collapse();
    if (kind === 'clean') {
      stopTimer();
      sn.setActivity(null);
      sn.setSecondary(null);
      sn.setPanel({ timer: { label: '25:00', running: false, pressed: false }, media: mediaPanel(track()) });
      termLines.forEach((l) => l.classList.remove('on'));
    } else if (kind === 'music') {
      stopTimer();
      sn.setActivity(musicAct(track()));
      sn.setSecondary(null);
      sn.setPanel({ timer: { label: '25:00', running: false, pressed: false }, media: mediaPanel(track()) });
    } else {
      startTimer();
      sn.setActivity(timerAct());
      sn.setSecondary(musicBubble());
      sn.setPanel({ timer: { label: fmt(stage.left), running: true, pressed: false }, media: mediaPanel(track()) });
    }
  }

  const CHAPTERS = [
    {
      label: 'Music',
      dur: 5200,
      run(sc) {
        baseline('clean');
        moveCursor(1010, 560);
        sc.at(250, () => {
          trackIdx++;
          const t = track();
          pn.setActivity(musicAct(t));
          pn.setPanel({ media: mediaPanel(t) });
          sn.setPanel({ media: mediaPanel(t) });
          sn.setActivity(musicAct(t));
          sn.peek({ art: t.art, accent: t.accent, title: t.title, subtitle: t.artist, trailing: { type: 'bars' }, duration: 3000, key: 'media' });
        });
      },
    },
    {
      label: 'Panel',
      dur: 5600,
      run(sc) {
        baseline('music');
        sc.at(150, () => moveCursor(650, 14));
        sc.at(950, () => sn.hover(true));
        sc.at(2100, () => {
          const r = sn.hitRect('timer');
          if (!r) return;
          const s = screen.getBoundingClientRect();
          moveCursor((r.left + r.width * 0.45 - s.left) / stageScale, (r.top + r.height * 0.5 - s.top) / stageScale);
        });
        sc.at(3000, () => {
          clickCursor();
          sn.setPanel({ timer: { pressed: true } });
        });
        sc.at(3220, () => {
          startTimer();
          sn.setPanel({ timer: { pressed: false, running: true, label: fmt(stage.left) } });
        });
        sc.at(4000, () => {
          moveCursor(940, 470);
          sn.hover(false);
        });
        sc.at(4150, () => {
          sn.setActivity(timerAct());
          sn.setSecondary(musicBubble());
        });
      },
    },
    { label: 'Timer', dur: 3200, run: () => baseline('timer') },
    {
      label: 'Download',
      dur: 3900,
      run(sc) {
        baseline('timer');
        sc.at(300, () =>
          sn.peek({ icon: 'download', accent: BLUE, title: 'Download complete', subtitle: 'notch-0.2.0.zip, 4.2 MB', trailing: { type: 'button', label: 'Open' }, duration: 3100 })
        );
      },
    },
    {
      label: 'Claude Code',
      dur: 4800,
      run(sc) {
        baseline('timer');
        termLines.forEach((l) => l.classList.remove('on'));
        sc.at(250, () => termLines[0].classList.add('on'));
        sc.at(1000, () => termLines[1].classList.add('on'));
        sc.at(1450, () =>
          sn.peek({ icon: 'sparkle', accent: CLAUDE, title: 'Claude finished', subtitle: 'Click to jump back to the terminal', duration: 3000 })
        );
      },
    },
    {
      label: 'Caps Lock',
      dur: 2700,
      run(sc) {
        baseline('timer');
        sc.at(300, () =>
          sn.peek({ icon: 'keyboard', accent: GREEN, title: 'Caps Lock', subtitle: 'On', trailing: { type: 'text', text: 'ON' }, duration: 1900, instant: true, key: 'locks' })
        );
      },
    },
    {
      label: 'Volume',
      dur: 3100,
      run(sc) {
        baseline('timer');
        const v = (frac) => sn.peek({ icon: 'volume', accent: PAPER, title: 'Volume', trailing: { type: 'level', frac }, duration: 2000, instant: true, key: 'osd' });
        sc.at(300, () => v(0.52));
        sc.at(750, () => v(0.6));
        sc.at(1100, () => v(0.68));
        sc.at(1400, () => v(0.74));
      },
    },
  ];

  const chipsEl = $('#chips');
  chipsEl.innerHTML = CHAPTERS.map(
    (c, i) => `<button class="chip" type="button" role="tab" aria-selected="false" data-i="${i}" style="--d:${c.dur}ms">${c.label}<i></i></button>`
  ).join('');
  const chips = $$('.chip', chipsEl);
  const pauseBtn = $('#pauseBtn');
  const sc = new Script();
  let current = 0;
  let userPaused = REDUCED;
  let visible = false;
  let started = false;

  function play(i, advance = true) {
    sc.clear();
    current = (i + CHAPTERS.length) % CHAPTERS.length;
    chips.forEach((c, j) => {
      c.setAttribute('aria-selected', String(j === current));
      const bar = c.querySelector('i');
      bar.style.animation = 'none';
      void bar.offsetWidth;
      bar.style.animation = '';
    });
    CHAPTERS[current].run(sc);
    if (advance) sc.at(CHAPTERS[current].dur, () => play(current + 1));
  }
  function setPaused(p) {
    userPaused = p;
    $('.stage').classList.toggle('paused', p);
    chipsEl.classList.toggle('paused', p);
    pauseBtn.setAttribute('aria-pressed', String(p));
    pauseBtn.querySelector('span').textContent = p ? 'Play' : 'Pause';
    pauseBtn.querySelector('svg').innerHTML = p
      ? '<path d="M8 5.5v13l10.5-6.5z" class="f"/>'
      : '<rect x="6.5" y="5" width="4" height="14" rx="1.3" class="f"/><rect x="13.5" y="5" width="4" height="14" rx="1.3" class="f"/>';
    if (p) sc.clear();
    else if (visible) play(current);
  }
  pauseBtn.addEventListener('click', () => setPaused(!userPaused));
  chipsEl.addEventListener('click', (e) => {
    const b = e.target.closest('.chip');
    if (b) play(+b.dataset.i, !userPaused);
  });

  function startStage() {
    if (started) return;
    started = true;
    setTimeout(() => sn.setShown(true), 300);
    if (userPaused) {
      setPaused(true);
      setTimeout(() => {
        play(1, false);
      }, 400);
    } else setTimeout(() => play(0), 700);
  }

  // --------------------------------------------------------- page notch

  const pageScale = () => (innerWidth < 560 ? 0.62 : innerWidth < 860 ? 0.8 : 1);
  const pn = new Notch($('#pageNotch'), { hidden: true, interactive: true, scale: pageScale() });
  pn.setActivity(musicAct(track()));
  pn.setPanel({ media: mediaPanel(track()), timer: { label: '25:00' } });
  addEventListener('resize', () => {
    pn.scale = pageScale();
    pn.draw();
  });
  // handy from the devtools console: DNPage.page.peek({ title: 'Hi' })
  window.DNPage = { stage: sn, page: pn };

  // The hero and the laptop section have notches of their own, so the page's
  // notch only drops in once you've scrolled past the laptop section.
  const statesEl = $('#states');
  const syncPageNotch = () => {
    const show = statesEl.getBoundingClientRect().bottom < innerHeight * 0.35;
    if (show === pn.shownFlag) return;
    pn.setShown(show);
    if (!show) {
      pn.clearPeeks();
      pn.collapse();
    }
  };
  let notchTick = false;
  addEventListener(
    'scroll',
    () => {
      if (notchTick) return;
      notchTick = true;
      requestAnimationFrame(() => {
        notchTick = false;
        syncPageNotch();
      });
    },
    { passive: true }
  );
  addEventListener('resize', syncPageNotch);
  syncPageNotch();

  new IntersectionObserver(
    ([e]) => {
      visible = e.isIntersecting;
      if (visible) {
        startStage();
        if (!userPaused && started) play(current);
      } else sc.clear();
    },
    { threshold: 0.12 }
  ).observe($('#stage'));

  // -------------------------------------------------------------- states

  {
    const L = window.ISO.laptop();
    const host = $('#laptop');
    // The detail view is the top strip of the screen. It starts glued to the
    // lid and peels off toward you as the section scrolls in.
    const DH = 150;
    host.innerHTML =
      L.svg +
      `<div class="laptop-screen" style="width:${L.w}px;height:${L.h}px;transform:translate(${-L.vb.x}px,${-L.vb.y}px) ${L.matrix}">
        <span class="ls-win"></span><span class="ls-win ls-win2"></span><span class="ls-bar"></span>
        <div class="laptop-notch" id="laptopNotch"></div></div>
      <div class="detail" id="detail" style="width:${L.w}px;height:${DH}px">
        <span class="ls-win"></span><span class="ls-win ls-win2"></span>
        <div class="laptop-notch" id="detailNotch"></div>
        <svg class="d-cursor" id="dCursor" viewBox="0 0 20 24" aria-hidden="true"><path d="M2 1.5v18.2l5-4.6 3.3 7.2 3-1.4-3.2-7H17z" fill="#fff" stroke="#111" stroke-width="1.4" stroke-linejoin="round"/></svg>
      </div>`;
    const readout = $('#dimReadout span');
    const ln = new Notch($('#laptopNotch'), { scale: 0.6 });
    const dn = new Notch($('#detailNotch'), {
      scale: 0.6,
      onFrame: (w, h) => (readout.textContent = `${Math.round(w)} × ${Math.round(h)}`),
    });
    const both = (fn) => [ln, dn].forEach(fn);
    const fit = $('#laptopFit');
    const stageBox = fit.parentElement;
    const resize = () => {
      const w = stageBox.clientWidth;
      const narrow = innerWidth < 860;
      const h = narrow ? innerHeight * 0.44 : stageBox.clientHeight - 40;
      const s = Math.min(w / 672, h / 705);
      fit.style.width = 672 * s + 'px';
      fit.style.height = 705 * s + 'px';
      host.style.setProperty('--ls', s);
    };
    new ResizeObserver(resize).observe(stageBox);
    addEventListener('resize', resize);

    // ---- peel-off: interpolate the strip's matrix from the lid to face-on
    const detail = $('#detail');
    const grid = $('.states-grid');
    const K = 1.55;
    const M0 = [L.C, L.S, 0, 1, L.o[0] - L.vb.x, L.o[1] - L.vb.y];
    const M1 = [K, 0, 0, K, (672 - L.w * K) / 2, 8];
    const smooth = (t) => t * t * (3 - 2 * t);
    let lastP = -1;
    const peel = () => {
      const r = grid.getBoundingClientRect();
      const vh = innerHeight;
      if (r.bottom < -100 || r.top > vh + 100) return;
      const raw = REDUCED ? 1 : Math.max(0, Math.min(1, (vh * 0.92 - r.top) / (vh * 0.5)));
      const p = smooth(raw);
      if (Math.abs(p - lastP) < 0.0005) return;
      lastP = p;
      const m = M0.map((v, i) => v + (M1[i] - v) * p);
      m[5] -= 46 * Math.sin(Math.PI * p);
      detail.style.transform = `matrix(${m.map((v) => v.toFixed(4)).join(',')})`;
      host.style.setProperty('--p', p.toFixed(3));
    };

    // ---- a cursor inside the strip
    const dCursor = $('#dCursor');
    const cur = (x, y, show = true) => {
      dCursor.style.transform = `translate(${x}px, ${y}px)`;
      dCursor.style.opacity = show ? 1 : 0;
    };
    const click = () => {
      dCursor.classList.remove('click');
      void dCursor.getBoundingClientRect();
      dCursor.classList.add('click');
    };

    // ---- each step acts out its own text, on a loop
    const loop = new Script();
    const reset = () => {
      loop.clear();
      both((n) => {
        n.clearPeeks();
        n.lean(false);
        n.collapse();
        n.setSecondary(null);
      });
    };
    const DEMO_PEEKS = [
      { icon: 'download', accent: BLUE, title: 'Download complete', subtitle: 'notch-0.2.0.zip, 4.2 MB', trailing: { type: 'check' } },
      { icon: 'bolt', accent: GREEN, title: 'Charger connected', subtitle: '64%, full in 1 h 10 min', trailing: { type: 'battery', level: 0.64, charging: true } },
      { icon: 'keyboard', accent: GREEN, title: 'Caps Lock', subtitle: 'On', trailing: { type: 'text', text: 'ON' } },
      () => {
        const t = TRACKS[1];
        return { art: t.art, accent: t.accent, title: t.title, subtitle: t.artist, trailing: { type: 'bars' } };
      },
    ];
    const STATES = {
      rest() {
        both((n) => n.setActivity(null));
        cur(300, 112);
        const cycle = () => {
          loop.at(500, () => cur(222, 9));
          loop.at(1250, () => both((n) => n.lean(true)));
          loop.at(2900, () => {
            cur(318, 118);
            both((n) => n.lean(false));
          });
          loop.at(4300, cycle);
        };
        cycle();
      },
      live() {
        cur(318, 118, false);
        let i = 0;
        let ringP = 1;
        both((n) => n.setActivity(musicAct(TRACKS[0])));
        loop.at(1300, () => both((n) => n.setSecondary({ type: 'ring', p: ringP, color: HONEY })));
        const tick = () => {
          i++;
          ringP = Math.max(0.1, ringP - 0.15);
          both((n) => {
            n.setActivity(musicAct(TRACKS[i % TRACKS.length]));
            n.setSecondary({ type: 'ring', p: ringP, color: HONEY });
          });
          loop.at(3000, tick);
        };
        loop.at(3400, tick);
      },
      peek() {
        cur(318, 118, false);
        both((n) => n.setActivity(musicAct(TRACKS[0])));
        let i = 0;
        const next = () => {
          const d = DEMO_PEEKS[i++ % DEMO_PEEKS.length];
          const p = typeof d === 'function' ? d() : d;
          both((n) => n.peek(Object.assign({}, p, { duration: 2300, instant: true })));
          loop.at(3100, next);
        };
        loop.at(350, next);
      },
      open() {
        both((n) => {
          n.setActivity(musicAct(TRACKS[0]));
          n.setPanel({ media: Object.assign(mediaPanel(TRACKS[0]), { playing: true }) });
        });
        cur(318, 118);
        const cycle = () => {
          loop.at(400, () => cur(214, 8));
          loop.at(1100, () => both((n) => n.hover(true)));
          loop.at(2000, () => cur(73, 82));
          loop.at(2800, () => {
            click();
            both((n) => n.setPanel({ media: { playing: false } }));
          });
          loop.at(3500, () => {
            click();
            both((n) => n.setPanel({ media: { playing: true } }));
          });
          loop.at(4300, () => {
            cur(330, 132);
            both((n) => n.hover(false));
          });
          loop.at(5800, cycle);
        };
        cycle();
      },
    };

    const steps = $$('#steps .step');
    let active = null;
    let inView = false;
    const run = () => {
      reset();
      if (inView && active) STATES[active.dataset.state]();
    };
    const activate = (step) => {
      if (step === active) return;
      active = step;
      steps.forEach((s) => s.classList.toggle('is-active', s === step));
      run();
    };
    activate(steps[0]);
    new IntersectionObserver(([e]) => {
      if (e.isIntersecting === inView) return;
      inView = e.isIntersecting;
      run();
    }).observe($('#states'));
    peel();
    addEventListener('resize', () => {
      lastP = -1;
      peel();
    });
    // the step whose text is closest to the middle of the screen wins
    const pick = () => {
      const mid = innerHeight * 0.5;
      let best = steps[0];
      let bestD = Infinity;
      for (const s of steps) {
        const r = s.querySelector('h3').getBoundingClientRect();
        const d = Math.abs(r.top + 40 - mid);
        if (d < bestD) {
          bestD = d;
          best = s;
        }
      }
      activate(best);
    };
    let pending = false;
    addEventListener(
      'scroll',
      () => {
        if (pending) return;
        pending = true;
        requestAnimationFrame(() => {
          pending = false;
          pick();
          peel();
        });
      },
      { passive: true }
    );
  }

  // ------------------------------------------------------------- modules

  const PEEKS = {
    media: () => {
      const t = track();
      return { art: t.art, accent: t.accent, title: t.title, subtitle: t.artist, trailing: { type: 'bars' } };
    },
    timer: () => ({ icon: 'timer', accent: HONEY, title: 'Timer started', subtitle: '25:00 remaining', trailing: { type: 'ring', p: 0.92 } }),
    downloads: () => ({ icon: 'download', accent: BLUE, title: 'Download complete', subtitle: 'notch-0.2.0.zip, 4.2 MB', trailing: { type: 'button', label: 'Open' } }),
    clipboard: () => ({ icon: 'clipboard', accent: PAPER, title: 'Copied', subtitle: 'cargo build --release', trailing: { type: 'check' } }),
    shelf: () => ({ icon: 'tray', accent: '#F06B5A', title: '3 files on the shelf', subtitle: 'Drag them out when you need them' }),
    voice: () => ({ icon: 'mic', accent: SEA, title: 'Listening', subtitle: 'Let go of Ctrl+Alt+D to type it', trailing: { type: 'wave' } }),
    ai: () => ({ icon: 'sparkle', accent: LILAC, title: '72°F is 22.2°C', subtitle: '(72 − 32) × 5 ÷ 9' }),
    privacy: () => ({ icon: 'camera', accent: ORANGE, title: 'Camera in use', subtitle: 'Video call', trailing: { type: 'dots', colors: [GREEN, ORANGE] } }),
    ports: () => ({ icon: 'globe', accent: GREEN, title: 'localhost:5173 is up', subtitle: 'node, started just now', trailing: { type: 'button', label: 'Open' } }),
    locks: () => ({ icon: 'keyboard', accent: GREEN, title: 'Caps Lock', subtitle: 'On', trailing: { type: 'text', text: 'ON' } }),
    battery: () => ({ icon: 'bolt', accent: GREEN, title: 'Charging', subtitle: '86%, full in 40 min', trailing: { type: 'battery', level: 0.86, charging: true } }),
    claude: () => ({ icon: 'sparkle', accent: CLAUDE, title: 'Claude finished', subtitle: 'Click to jump back to the terminal' }),
  };

  const tiles = $$('#moduleGrid .tile');
  tiles.forEach((tile) => {
    const name = tile.dataset.module;
    const make = window.ISO.objects[name];
    if (make) $('.tile-art', tile).innerHTML = make();
    let leaveT = 0;
    const on = () => {
      clearTimeout(leaveT);
      tiles.forEach((t) => t !== tile && t.classList.remove('is-active'));
      if (tile.classList.contains('is-active')) return;
      tile.classList.add('is-active');
      pn.peek(Object.assign(PEEKS[name](), { key: 'module', instant: true, duration: 3200 }));
    };
    const off = () => {
      leaveT = setTimeout(() => tile.classList.remove('is-active'), 120);
    };
    tile.addEventListener('mouseenter', on);
    tile.addEventListener('mouseleave', off);
    const hit = $('.tile-hit', tile);
    hit.addEventListener('focus', on);
    hit.addEventListener('blur', off);
    hit.addEventListener('click', () => {
      tile.classList.remove('is-active');
      on();
    });
  });

  // "Look up." — the notch offers the download itself
  let ctaAt = 0;
  new IntersectionObserver(
    ([e]) => {
      if (!e.isIntersecting || !pn.shownFlag || Date.now() - ctaAt < 20000) return;
      ctaAt = Date.now();
      pn.peek({
        icon: 'download',
        accent: PAPER,
        title: 'Dynamic Notch for Windows',
        subtitle: 'Free, about 1 MB. Click to download.',
        trailing: { type: 'button', label: 'Get' },
        duration: 6000,
        key: 'cta',
        instant: true,
        href: 'https://github.com/eigenes/DynamicNotch/releases/latest',
      });
    },
    { threshold: 0.9 }
  ).observe($('.closing h2'));

  // ----------------------------------------------------------- scripting

  const COMMANDS = [
    {
      title: 'Tell you a build finished',
      cmd: 'dynamic-notch notify "Build finished" "All tests passed"',
      run: () => pn.peek({ icon: 'check', accent: GREEN, title: 'Build finished', subtitle: 'All tests passed', instant: true, key: 'cmd' }),
    },
    {
      title: 'Show progress',
      cmd: 'dynamic-notch progress "Export video" 42',
      run(sc2) {
        const p = (v, sub) =>
          pn.peek({ icon: 'file', accent: BLUE, title: 'Export video', subtitle: sub, trailing: v < 1 ? { type: 'ring', p: v } : { type: 'check' }, instant: true, key: 'cmd', duration: 2600 });
        p(0.42, '42%');
        sc2.at(800, () => p(0.71, '71%'));
        sc2.at(1500, () => p(1, 'Done'));
      },
    },
    {
      title: 'Start a timer',
      cmd: 'dynamic-notch timer 25',
      run: () => pn.peek({ icon: 'timer', accent: HONEY, title: 'Timer started', subtitle: '25:00', trailing: { type: 'ring', p: 1 }, instant: true, key: 'cmd' }),
    },
    {
      title: 'Ask a quick question',
      cmd: 'dynamic-notch ask "convert 72F to C"',
      run: () => pn.peek({ icon: 'sparkle', accent: LILAC, title: '72°F is 22.2°C', subtitle: '(72 − 32) × 5 ÷ 9', instant: true, key: 'cmd' }),
    },
    {
      title: 'Forward a Claude Code event',
      cmd: 'dynamic-notch claude stop "Tests pass"',
      run: () => pn.peek({ icon: 'sparkle', accent: CLAUDE, title: 'Claude finished', subtitle: 'Tests pass', instant: true, key: 'cmd' }),
    },
  ];

  const cmdList = $('#cmdList');
  const out = $('#consoleOut');
  const esc = (s) => s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
  const PROMPT = '<span class="p">PS C:\\Users\\you&gt;</span> ';
  let history = [];
  const cmdScript = new Script();
  const renderOut = (typing = '') => {
    out.innerHTML = history.map((h) => `${PROMPT}${esc(h)}\n`).join('') + PROMPT + esc(typing) + '<span class="caret"></span>';
  };
  renderOut();
  cmdList.innerHTML = COMMANDS.map(
    (c, i) => `<button class="cmd" type="button" data-i="${i}"><b>${c.title}</b><span>${esc(c.cmd)}</span></button>`
  ).join('');
  cmdList.addEventListener('click', (e) => {
    const b = e.target.closest('.cmd');
    if (!b) return;
    const c = COMMANDS[+b.dataset.i];
    $$('.cmd', cmdList).forEach((x) => x.classList.toggle('is-on', x === b));
    cmdScript.clear();
    const step = REDUCED ? c.cmd.length : 2;
    let n = 0;
    const type = () => {
      n = Math.min(c.cmd.length, n + step);
      renderOut(c.cmd.slice(0, n));
      if (n < c.cmd.length) cmdScript.at(16, type);
      else
        cmdScript.at(180, () => {
          history = history.concat(c.cmd).slice(-4);
          renderOut();
          c.run(cmdScript);
        });
    };
    type();
  });

  // ------------------------------------------------------- performance

  {
    const fig = $('#layersFig');
    const Ls = window.ISO.layers();
    const LX = 340;
    const labelY = [44, -36, -116, -196];
    let svgInner = '';
    Ls.forEach((l, i) => {
      svgInner += `<g class="lyr" data-i="${i}">${l.s}</g>`;
    });
    Ls.forEach((l, i) => {
      svgInner += `<g class="lbl"><line class="lead" data-i="${i}" stroke="rgba(239,232,220,.28)" stroke-width="1"/>
        <circle class="lead-dot" data-i="${i}" r="2.6" fill="${PAPER}"/>
        <text class="lbl-name" x="${LX + 14}" y="${labelY[i] - 5}">${l.label}</text>
        <text class="lbl-api" x="${LX + 14}" y="${labelY[i] + 18}">${l.api}</text></g>`;
    });
    fig.innerHTML = `<svg xmlns="http://www.w3.org/2000/svg">${svgInner}</svg>`;
    const svgEl = $('svg', fig);
    // narrow screens: crop to the stack, the HTML list below takes the labels
    const fitBox = () => svgEl.setAttribute('viewBox', innerWidth < 860 ? '-320 -360 650 570' : '-320 -360 990 570');
    fitBox();
    addEventListener('resize', fitBox);
    const groups = $$('.lyr', fig);
    const leads = $$('.lead', fig);
    const dots = $$('.lead-dot', fig);

    const setGap = (gap) => {
      groups.forEach((g, i) => g.setAttribute('transform', `translate(0 ${-i * gap})`));
      Ls.forEach((l, i) => {
        const ax = l.anchor[0];
        const ay = l.anchor[1] - i * gap;
        leads[i].setAttribute('x1', ax + 6);
        leads[i].setAttribute('y1', ay);
        leads[i].setAttribute('x2', LX);
        leads[i].setAttribute('y2', labelY[i]);
        dots[i].setAttribute('cx', ax + 6);
        dots[i].setAttribute('cy', ay);
      });
    };
    const ease = (t) => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);
    let ticking = false;
    const update = () => {
      ticking = false;
      const r = fig.getBoundingClientRect();
      const vh = innerHeight;
      if (r.bottom < -200 || r.top > vh + 200) return;
      const center = r.top + r.height / 2;
      const p = Math.max(0, Math.min(1, (vh * 0.95 - center) / (vh * 0.55)));
      setGap(REDUCED ? 80 : 10 + 70 * ease(p));
    };
    addEventListener(
      'scroll',
      () => {
        if (!ticking) {
          ticking = true;
          requestAnimationFrame(update);
        }
      },
      { passive: true }
    );
    addEventListener('resize', update);
    setGap(REDUCED ? 80 : 10);
    update();
  }

  // frame graph: counts frames the notch engines on this page draw
  {
    const canvas = $('#framesCanvas');
    const ctx = canvas.getContext('2d');
    const now = $('#fpsNow');
    const N = 160;
    const samples = new Array(N).fill(0);
    let lastFrames = Pacer.frames;
    let iv = 0;
    const draw = () => {
      const dpr = Math.min(2, devicePixelRatio || 1);
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== Math.round(w * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(h * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const peak = Math.max(...samples);
      const top = peak > 120 ? 180 : peak > 60 ? 120 : 60;
      ctx.font = '11px "Geist Mono", monospace';
      ctx.textBaseline = 'middle';
      for (let i = 0; i <= 2; i++) {
        const y = 6 + ((h - 12) * i) / 2;
        ctx.strokeStyle = 'rgba(239,232,220,0.08)';
        ctx.beginPath();
        ctx.moveTo(0, Math.round(y) + 0.5);
        ctx.lineTo(w - 34, Math.round(y) + 0.5);
        ctx.stroke();
        ctx.fillStyle = 'rgba(239,232,220,0.35)';
        ctx.fillText(String(Math.round(top * (1 - i / 2))), w - 28, y);
      }
      const gw = w - 34;
      const step = gw / (N - 1);
      const yOf = (v) => 6 + (h - 12) * (1 - Math.min(v, top) / top);
      ctx.beginPath();
      ctx.moveTo(0, yOf(samples[0]));
      for (let i = 1; i < N; i++) {
        ctx.lineTo(i * step, yOf(samples[i - 1]));
        ctx.lineTo(i * step, yOf(samples[i]));
      }
      ctx.strokeStyle = SEA;
      ctx.lineWidth = 1.5;
      ctx.stroke();
      ctx.lineTo(gw, h - 6);
      ctx.lineTo(0, h - 6);
      ctx.closePath();
      ctx.fillStyle = 'rgba(127,200,181,0.14)';
      ctx.fill();
    };
    const sample = () => {
      const f = Pacer.frames;
      const fps = (f - lastFrames) * 4;
      lastFrames = f;
      samples.push(fps);
      samples.shift();
      now.textContent = fps;
      draw();
    };
    new IntersectionObserver(([e]) => {
      clearInterval(iv);
      if (e.isIntersecting) {
        lastFrames = Pacer.frames;
        iv = setInterval(sample, 250);
        draw();
      }
    }).observe(canvas);
    addEventListener('resize', draw);
  }
})();
