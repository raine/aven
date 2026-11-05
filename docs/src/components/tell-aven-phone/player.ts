// Plays a scripted Tell Aven session on the phone and applies its changes to the
// queue in the surrounding quick capture section.
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
const inView = (element: HTMLElement) => new Promise<void>(resolve => {
  const observer = new IntersectionObserver(entries => {
    if (entries.some(entry => entry.isIntersecting)) { observer.disconnect(); resolve(); }
  }, { threshold: 0.4 });
  observer.observe(element);
});

// Dictation as the on-device recognizer reports it: the current phrase arrives as
// volatile text that grows and gets revised, then settles whole after a pause.
type TranscriptEvent = { at: number; finalized?: string; volatile: string };
// Scales the scripted speech below; under 1 speaks faster.
const inputPace = 0.7;
const transcriptEvents: TranscriptEvent[] = (<TranscriptEvent[]>[
  { at: 750, volatile: 'Add a' },
  { at: 1100, volatile: 'Add a task to' },
  { at: 1450, volatile: 'Add a task to rotate the' },
  { at: 1850, volatile: 'Add a task to rotate the stating' },
  { at: 2200, volatile: 'Add a task to rotate the staging API' },
  { at: 2650, volatile: 'Add a task to rotate the staging API keys' },
  { at: 3250, finalized: 'Add a task to rotate the staging API keys', volatile: '' },
  { at: 3700, volatile: 'before Friday' },
  { at: 4150, volatile: 'before Friday’s release' },
  { at: 4950, finalized: 'Add a task to rotate the staging API keys before Friday’s release', volatile: '' },
  { at: 5450, volatile: 'and label' },
  { at: 5850, volatile: 'and label it security' },
  { at: 6700, finalized: 'Add a task to rotate the staging API keys before Friday’s release and label it security.', volatile: '' },
]).map(event => ({ ...event, at: event.at * inputPace }));
const speaking = ([[300, 2750], [3350, 4300], [5150, 6100]] as const).map(([from, to]) => [from * inputPace, to * inputPace]);
const listenFor = 7300 * inputPace;

// Speech as syllables laid out over each speaking interval: uneven lengths and
// loudness, quieter function words, short dips between words.
const spokenPhrases = ['Add a task to rotate the staging API keys', 'before Friday’s release', 'and label it security'];
const quietWords = new Set(['a', 'to', 'the', 'it', 'and']);
function speechSyllables() {
  const syllables: { from: number; to: number; peak: number }[] = [];
  spokenPhrases.forEach((phrase, i) => {
    const [from, to] = speaking[i];
    const planned: { gap: number; length: number; peak: number }[] = [];
    for (const word of phrase.split(' ')) {
      const count = /^[A-Z]{2,}$/.test(word) ? word.length : Math.max(1, word.toLowerCase().match(/[aeiouy]+/g)?.length ?? 1);
      const quiet = quietWords.has(word.toLowerCase());
      for (let n = 0; n < count; n++) {
        const stressed = n === 0 && !quiet;
        planned.push({
          gap: n === 0 ? 70 + Math.random() * 90 : 0,
          length: (quiet ? 90 : 140) + Math.random() * 140,
          peak: Math.min(1, (quiet ? 0.25 : 0.45) + Math.random() * (quiet ? 0.25 : 0.45) + (stressed ? 0.1 : 0)),
        });
      }
    }
    // Word gaps keep their length so they still read as dips; syllables absorb the rest.
    const gaps = planned.reduce((sum, item) => sum + item.gap, 0);
    const stretch = Math.max(0.3, (to - from - gaps) / planned.reduce((sum, item) => sum + item.length, 0));
    let t = from;
    for (const item of planned) {
      t += item.gap;
      syllables.push({ from: t, to: t + item.length * stretch, peak: item.peak });
      t += item.length * stretch;
    }
  });
  return syllables;
}

// Mirrors SpeechLevelMeter and SpeechWaveform: 15 levels a second, smoothed with a
// fast attack and slower release, drawn as bars that enter right and scroll left.
function waveform(canvas: HTMLCanvasElement, start: number) {
  const ctx = canvas.getContext('2d')!;
  const dpr = devicePixelRatio || 1;
  const rate = 15, bar = 2, step = 4;
  const levels: number[] = [];
  let smoothed = 0, frame = 0;
  const syllables = speechSyllables();
  const level = (t: number) => {
    const syllable = syllables.find(({ from, to }) => t >= from && t < to);
    if (syllable) {
      const u = (t - syllable.from) / (syllable.to - syllable.from);
      // Quick onset, slower decay, like a voiced syllable.
      const envelope = u < 0.25 ? u / 0.25 : 1 - ((u - 0.25) / 0.75) ** 1.4;
      return Math.max(0, syllable.peak * (0.15 + 0.85 * envelope) + (Math.random() - 0.5) * 0.1);
    }
    const talking = speaking.some(([from, to]) => t >= from && t <= to);
    if (talking) return 0.03 + Math.random() * 0.07;
    // Room noise: mostly dots, now and then a faint breath.
    return Math.random() < 0.06 ? 0.1 + Math.random() * 0.08 : Math.random() * 0.05;
  };
  const draw = (now: number) => {
    if (!canvas.isConnected || !canvas.closest('.is-listening')) return;
    const t = now - start;
    const head = t / 1000 * rate;
    while (levels.length <= head) {
      // Like the meter's RMS over each slice of audio, not a single point.
      const sliceStart = levels.length / rate * 1000;
      const value = Math.sqrt([0, 1, 2, 3, 4].reduce((sum, k) => sum + level(sliceStart + k * 1000 / rate / 5) ** 2, 0) / 5);
      smoothed += (value - smoothed) * (value > smoothed ? 0.7 : 0.35);
      levels.push(smoothed);
    }
    const width = canvas.clientWidth, height = canvas.clientHeight;
    if (canvas.width !== width * dpr) { canvas.width = width * dpr; canvas.height = height * dpr; }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = '#5c5c6a';
    const newest = Math.floor(head);
    const offset = Math.round((head - newest) * step * dpr) / dpr;
    for (let back = 0; back < width / step + 2 && newest - back >= 0; back++) {
      const h = Math.max(bar, Math.round(levels[newest - back] * height * dpr) / dpr);
      const x = width - bar - back * step - offset;
      ctx.beginPath();
      ctx.roundRect(x, (height - h) / 2, bar, h, bar / 2);
      ctx.fill();
    }
    frame = requestAnimationFrame(draw);
  };
  frame = requestAnimationFrame(draw);
  return () => cancelAnimationFrame(frame);
}

// Dates as the app shows them ("Fri, Nov 13"): the coming Friday and the Monday after.
function showDates(phone: HTMLElement) {
  const friday = new Date();
  friday.setDate(friday.getDate() + ((5 - friday.getDay() + 7) % 7 || 7));
  const monday = new Date(friday);
  monday.setDate(friday.getDate() + 3);
  const format = (date: Date) => date.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric' });
  phone.querySelectorAll<HTMLElement>('[data-date]').forEach(el => {
    el.textContent = format(el.dataset.date === 'monday' ? monday : friday);
  });
}

export async function play(phone: HTMLElement) {
  showDates(phone);
  const section = phone.closest<HTMLElement>('.capture-section')!;
  const row = section.querySelector<HTMLElement>('.capture-fresh');
  const updated = section.querySelector<HTMLElement>('[data-ref="AVN-WV80"]');
  const priority = updated?.querySelector<HTMLElement>('[data-priority]');
  const settled = phone.querySelector<HTMLElement>('.tp-settled')!;
  const volatile = phone.querySelector<HTMLElement>('.tp-volatile')!;
  const typed = phone.querySelector<HTMLElement>('.tp-typed')!;
  const canvas = phone.querySelector<HTMLCanvasElement>('.tp-wave')!;
  const thread = phone.querySelector<HTMLElement>('.tp-thread')!;
  const scroller = phone.querySelector<HTMLElement>('.tp-scroll')!;
  const followUp = phone.dataset.followUp!;
  const alive = () => phone.isConnected;
  const stages = ['is-listening', 'is-typing', 'has-text', 'is-sent', 'is-step1', 'is-step1-done', 'is-step2', 'is-drafted', 'is-pressing', 'is-pressing-send', 'is-applied',
    'is-sent2', 'is-step3', 'is-step3-done', 'is-step4', 'is-drafted2', 'is-applied2'];
  // Keeps the newest message in view, as the thread scrolls on the device.
  const follow = () => requestAnimationFrame(() => {
    const overflow = Math.max(0, scroller.scrollHeight - thread.clientHeight);
    scroller.style.transform = overflow ? `translateY(${-overflow}px)` : '';
    thread.classList.toggle('is-scrolled', overflow > 0);
  });
  const add = (...names: string[]) => { phone.classList.add(...names); follow(); };
  const remove = (...names: string[]) => phone.classList.remove(...names);
  const setUpdated = (on: boolean) => {
    updated?.classList.toggle('capture-updated', on);
    if (priority) priority.textContent = on ? '!' : '●';
  };
  const press = async (name: string) => { add(name); await wait(170); remove(name); };

  // The new row stays hidden until the session applies it.
  section.classList.add('capture-playing');
  while (alive()) {
    // Each run starts from the top when the phone is in view.
    await inView(phone);
    remove(...stages);
    follow();
    row?.classList.remove('is-in');
    setUpdated(false);
    settled.textContent = volatile.textContent = typed.textContent = '';
    await wait(900);

    // Turn 1: speak a new task.
    add('is-listening');
    const start = performance.now();
    const stopWave = waveform(canvas, start);
    let finalized = '';
    for (const event of transcriptEvents) {
      await wait(Math.max(0, event.at - (performance.now() - start)));
      if (!alive()) return;
      if (event.finalized !== undefined) finalized = event.finalized;
      settled.textContent = finalized;
      volatile.textContent = event.volatile && finalized ? ' ' + event.volatile : event.volatile;
      phone.classList.toggle('has-text', Boolean(finalized || event.volatile));
    }
    await wait(Math.max(0, listenFor - (performance.now() - start)));
    await press('is-pressing-send');
    stopWave();
    remove('is-listening', 'has-text');
    add('is-sent');
    await wait(500);
    add('is-step1');
    await wait(900);
    add('is-step1-done', 'is-step2');
    await wait(1000);
    add('is-drafted');
    await wait(2200);
    await press('is-pressing');
    add('is-applied');
    await wait(350);
    row?.classList.add('is-in');
    await wait(2200);

    // Turn 2: type a follow-up that changes an existing task.
    add('is-typing');
    for (let i = 1; i <= followUp.length && alive(); i++) {
      typed.textContent = followUp.slice(0, i);
      if (i === 1) add('has-text');
      await wait(28 + Math.random() * 45 + (followUp[i - 1] === ' ' ? 40 : 0));
    }
    await wait(500);
    await press('is-pressing-send');
    remove('is-typing', 'has-text');
    typed.textContent = '';
    add('is-sent2');
    await wait(500);
    add('is-step3');
    await wait(1000);
    add('is-step3-done', 'is-step4');
    await wait(900);
    add('is-drafted2');
    await wait(2600);
    await press('is-pressing');
    add('is-applied2');
    await wait(350);
    setUpdated(true);
    await wait(6000);
  }
}
