// Plays the agent sessions: types and streams the transcript, and animates the queue as
// each aven command lands. Without motion the server-rendered scene stays as it is, and the
// scene list jumps between scenes.
import { FX, fxNames, layout, queueBefore, CELL_CLASSES, type Item, type Queue } from './story';

const motionOK = () => matchMedia('(prefers-reduced-motion: no-preference)').matches;
// Pause before each kind of step, so the reader can finish the previous line.
const PAUSE = { user: 600, tool: 200, stream: 250, line: 400 };
const SLIDE = 'transform 420ms cubic-bezier(0.23, 1, 0.32, 1)';

type Kind = keyof typeof PAUSE;

class Stale extends Error {}

class Player {
  private visible = true;
  private held: (() => void)[] = [];
  private stopped = false;
  private observer?: IntersectionObserver;
  private q: Queue;

  constructor(private el: HTMLElement, private from: number) {
    this.q = queueBefore(from);
  }

  // Timers stop while the section is off screen and resume where they left off.
  wait(ms: number) {
    return new Promise<void>((resolve, reject) => {
      const tick = () => {
        if (this.stopped) return reject(new Stale());
        if (!this.visible) return this.held.push(tick);
        resolve();
      };
      setTimeout(tick, ms);
    });
  }

  start() {
    this.el.setAttribute('data-motion', '');
    this.showChapter(this.from);
    this.el.querySelector('.q-list')!.replaceChildren();
    this.renderQueue(false);
    const target = this.el.querySelector<HTMLElement>('.terminal')!;
    const threshold = Math.min(0.6, (innerHeight * 0.85) / target.offsetHeight);
    let started = false;
    this.observer = new IntersectionObserver((entries) => {
      this.visible = entries.at(-1)!.isIntersecting;
      if (this.visible && !started) {
        started = true;
        this.play().catch((e) => { if (!(e instanceof Stale)) throw e; });
      }
      if (this.visible) this.held.splice(0).forEach((tick) => tick());
    }, { threshold });
    this.observer.observe(target);
  }

  stop() {
    this.stopped = true;
    this.observer?.disconnect();
  }

  showChapter(i: number) {
    const tabs = this.el.querySelectorAll<HTMLElement>('[data-tab]');
    tabs.forEach((b, j) => b.setAttribute('aria-selected', String(i === j)));
    const chapters = this.el.querySelectorAll<HTMLElement>('.chapter');
    chapters.forEach((c, j) => c.classList.toggle('on', i === j));
    this.el.querySelector('.session-title')!.textContent = chapters[i].dataset.title!;
    // Keeps the current tab in view when the list scrolls sideways (mobile).
    const nav = tabs[i].closest<HTMLElement>('.scene-list')!;
    if (nav.scrollWidth > nav.clientWidth) {
      nav.scrollBy({ left: tabs[i].getBoundingClientRect().left - nav.getBoundingClientRect().left, behavior: 'smooth' });
    }
  }

  progress(i: number, p: number) {
    this.el.querySelectorAll<HTMLElement>('[data-tab]')[i]?.style.setProperty('--p', String(p));
  }

  async play() {
    const chapters = [...this.el.querySelectorAll<HTMLElement>('.chapter')];
    const steps = chapters.flatMap((c, i) => (i < this.from ? [] : [...c.querySelectorAll<HTMLElement>('.step')].map((s) => ({ s, i }))));
    const totals = chapters.map((c) => c.querySelectorAll('.step').length);
    const chars = new Map(steps.map(({ s }) => [s, s.matches('.session-user') ? split(s.querySelector('.type')!) : s.matches('.stream') ? split(s) : []]));
    let doneIn = 0;
    for (const [n, { s: step, i }] of steps.entries()) {
      const kind: Kind = step.matches('.session-user') ? 'user' : step.matches('.session-tool') ? 'tool' : step.matches('.stream') ? 'stream' : 'line';
      if (step.dataset.ch && n > 0) {
        await this.wait(1500);
        this.progress(i - 1, 0);
        this.showChapter(i);
        doneIn = 0;
      }
      await this.wait(n === 0 ? 200 : PAUSE[kind]);
      step.classList.add('in');
      follow(step);
      if (kind === 'user') {
        if (step.matches('.typed')) { await this.wait(150); await this.type(chars.get(step)!); }
        else chars.get(step)!.forEach((c) => c.classList.add('on'));
      } else if (kind === 'stream') {
        await this.stream(chars.get(step)!);
      } else if (kind === 'tool') {
        for (const l of step.querySelectorAll('.l')) { await this.wait(70); l.classList.add('on'); }
        await this.wait(Number(step.dataset.ms));
        for (const r of step.querySelectorAll<HTMLElement>('.result')) {
          r.classList.add('on');
          for (const name of fxNames(r.dataset.fx)) { await this.fx(name); await this.wait(250); }
          await this.wait(60);
        }
      } else {
        await this.wait(400);
      }
      step.classList.add('done');
      this.progress(i, ++doneIn / totals[i]);
      for (const [k, name] of fxNames(step.dataset.fx).entries()) { await this.wait(k ? 900 : 150); await this.fx(name); }
    }
    await this.wait(6000);
    restart(this.el, 0);
  }

  async type(chars: HTMLElement[]) {
    for (let i = 0; i < chars.length; i++) {
      chars[i - 1]?.classList.remove('caret');
      chars[i].classList.add('on', 'caret');
      await this.wait(chars[i].textContent === ' ' ? 35 : 14 + Math.random() * 22);
    }
    await this.wait(250);
    chars.at(-1)?.classList.remove('caret');
  }

  // Streams in bursts: quick words, a stall after each sentence or clause.
  async stream(chars: HTMLElement[]) {
    let i = 0;
    while (i < chars.length) {
      let j = i;
      while (j < chars.length && chars[j].textContent !== ' ') j++;
      j = Math.min(j + 1, chars.length);
      for (let k = i; k < j; k++) chars[k].classList.add('on');
      const word = chars.slice(i, j).map((c) => c.textContent).join('').trim();
      i = j;
      await this.wait(/[.:,?]$/.test(word) ? 160 + Math.random() * 60 : 18 + Math.random() * 14);
    }
  }

  async fx(name: string) {
    const effects = FX[name](this.q);
    await this.renderQueue(true);
    effects.flash?.forEach((r) => this.mark(r, 'flash'));
    effects.peek?.forEach((r) => this.mark(r, 'peek'));
  }

  mark(ref: string, cls: string) {
    const row = this.el.querySelector(`.q-list [data-key="${ref}"]`);
    if (!row) return;
    row.classList.remove('flash', 'peek');
    void (row as HTMLElement).offsetWidth;
    row.classList.add(cls);
  }

  // Rows are keyed by ref, so a task that changes band slides to its new place (FLIP).
  async renderQueue(animate: boolean) {
    const list = this.el.querySelector<HTMLElement>('.q-list')!;
    const items = layout(this.q);
    const keys = new Set(items.map((i) => i.key));
    const old = new Map([...list.children].map((n) => [(n as HTMLElement).dataset.key!, n as HTMLElement]));
    const gone = [...old.values()].filter((n) => !keys.has(n.dataset.key!));
    if (animate && gone.length) {
      gone.forEach((n) => n.classList.add('leaving'));
      await this.wait(200);
    }
    const before = new Map<string, number>();
    if (animate) old.forEach((n, k) => keys.has(k) && before.set(k, n.getBoundingClientRect().top));
    gone.forEach((n) => n.remove());
    const nodes = items.map((item) => {
      let n = old.get(item.key);
      const fresh = !n;
      if (!n) {
        n = document.createElement('div');
        n.dataset.key = item.key;
        n.className = 'text' in item ? 'q-sect' : 'q-row';
        if (animate) {
          const entering = n;
          entering.classList.add('entering');
          entering.addEventListener('animationend', () => entering.classList.remove('entering'), { once: true });
        }
      }
      update(n, item, animate && !fresh);
      return n;
    });
    list.append(...nodes);
    this.el.querySelector('.q-count')!.textContent = `queue ${this.q.tasks.length}`;
    if (!animate) return;
    const moved = nodes.filter((n) => {
      const top = before.get(n.dataset.key!);
      if (top == null) return false;
      const dy = top - n.getBoundingClientRect().top;
      if (!dy) return false;
      n.style.transition = 'none';
      n.style.transform = `translateY(${dy}px)`;
      n.style.zIndex = n.classList.contains('q-row') ? '2' : '1';
      return true;
    });
    void list.offsetHeight;
    moved.forEach((n) => {
      n.style.transition = SLIDE;
      n.style.transform = '';
      n.addEventListener('transitionend', () => { n.style.transition = ''; n.style.zIndex = ''; }, { once: true });
    });
  }
}

function update(n: HTMLElement, item: Item, pop: boolean) {
  if ('text' in item) { n.textContent = item.text; return; }
  if (!n.children.length) n.innerHTML = CELL_CLASSES.map((c) => `<span class="${c}"></span>`).join('');
  item.cells.forEach((html, i) => {
    const c = n.children[i] as HTMLElement;
    if (c.dataset.h === html) return;
    c.dataset.h = html;
    // Marker and status changes pop so the eye catches them.
    c.innerHTML = pop && (i === 2 || i === 3) && html ? `<span class="pop">${html}</span>` : html;
  });
  n.classList.toggle('sel', item.sel);
}

// Scrolls the window once per step so the whole step is in view.
function follow(node: HTMLElement) {
  const feed = node.closest<HTMLElement>('.feed')!;
  const vp = feed.parentElement!;
  const bottom = node.getBoundingClientRect().bottom - feed.getBoundingClientRect().top;
  const shift = Math.max(0, Math.ceil(bottom + parseFloat(getComputedStyle(feed).paddingBottom) - vp.clientHeight));
  feed.style.transform = `translateY(${-shift}px)`;
  vp.classList.toggle('scrolled', shift > 0);
}

// Wraps every character in its own span; layout stays final while characters reveal.
function split(root: HTMLElement) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) if (walker.currentNode.nodeValue!.trim()) nodes.push(walker.currentNode as Text);
  for (const node of nodes) {
    const frag = document.createDocumentFragment();
    for (const c of node.nodeValue!.replace(/\s+/g, ' ')) {
      const s = document.createElement('span');
      s.className = 'ch';
      s.textContent = c;
      frag.append(s);
    }
    node.replaceWith(frag);
  }
  return [...root.querySelectorAll<HTMLElement>('.ch')];
}

// ---------- Section wiring ----------

const pristine = new WeakMap<HTMLElement, HTMLElement>();
const players = new WeakMap<HTMLElement, Player>();

// Replaces the section with an untouched copy and plays it from scene `from`.
function restart(el: HTMLElement, from: number) {
  players.get(el)?.stop();
  const fresh = pristine.get(el)!.cloneNode(true) as HTMLElement;
  el.replaceWith(fresh);
  arm(fresh, from, pristine.get(el));
}

// Without motion: shows scene i in full, with the queue as that session left it.
function jump(el: HTMLElement, i: number) {
  const tabs = el.querySelectorAll<HTMLElement>('[data-tab]');
  tabs.forEach((b, j) => b.setAttribute('aria-selected', String(i === j)));
  const chapters = el.querySelectorAll<HTMLElement>('.chapter');
  chapters.forEach((c, j) => c.classList.toggle('on', i === j));
  el.querySelector('.session-title')!.textContent = chapters[i].dataset.title!;
  const q = queueBefore(i + 1);
  const list = el.querySelector<HTMLElement>('.q-list')!;
  list.replaceChildren(...layout(q).map((item) => {
    const n = document.createElement('div');
    n.dataset.key = item.key;
    n.className = 'text' in item ? 'q-sect' : 'q-row';
    update(n, item, false);
    return n;
  }));
  el.querySelector('.q-count')!.textContent = `queue ${q.tasks.length}`;
}

export function arm(el: HTMLElement, from = 0, template?: HTMLElement) {
  pristine.set(el, template ?? (el.cloneNode(true) as HTMLElement));
  // A scene click plays the story from that scene, with the queue as the earlier scenes left it.
  el.querySelectorAll<HTMLElement>('[data-tab]').forEach((tab) => tab.addEventListener('click', () => {
    const i = Number(tab.dataset.tab);
    if (motionOK()) restart(el, i);
    else jump(el, i);
  }));
  if (!motionOK()) return;
  const player = new Player(el, from);
  players.set(el, player);
  player.start();
}
