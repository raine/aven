import { base, openCount, queueHTML, replyHTML, scenes, statusText, STATIC_SCENE, userHTML } from './story';

export function arm(section: HTMLElement) {
  const feed = section.querySelector<HTMLElement>('[data-feed]')!;
  const queue = section.querySelector<HTMLElement>('[data-queue]')!;
  const count = section.querySelector<HTMLElement>('[data-count]')!;
  const buttons = [...section.querySelectorAll<HTMLButtonElement>('[data-scene]')];
  const motion = window.matchMedia('(prefers-reduced-motion: no-preference)');
  let timers: number[] = [];

  const renderQueue = (rows: ReturnType<typeof base>) => {
    queue.innerHTML = queueHTML(rows);
    count.textContent = openCount(rows);
  };
  const add = (className: string, html: string) => {
    const element = document.createElement('div');
    element.className = `${className} mcp-enter`;
    element.innerHTML = html;
    feed.append(element);
  };
  const play = (index: number) => {
    timers.forEach(clearTimeout);
    timers = [];
    buttons.forEach((button, i) => button.setAttribute('aria-pressed', String(i === index)));
    const scene = scenes[index];
    const steps = [
      () => add('mcp-user', userHTML(scene)),
      () => add('mcp-status', 'Working'),
      () => {
        const status = feed.querySelector<HTMLElement>('.mcp-status')!;
        status.textContent = statusText(scene);
        status.dataset.done = '';
        status.toggleAttribute('data-ruled', Boolean(scene.expandable));
        renderQueue(scene.apply(base()));
        add('mcp-result', scene.result);
        add('mcp-reply', replyHTML(scene));
      },
    ];
    feed.innerHTML = '';
    renderQueue(base());
    steps.forEach((step, i) => {
      if (motion.matches) timers.push(window.setTimeout(step, 150 + i * 800));
      else step();
    });
  };

  buttons.forEach((button, i) => button.addEventListener('click', () => play(i)));

  // The server-rendered scene stands in until the section first scrolls into view.
  const observer = new IntersectionObserver((entries) => {
    if (!entries.some((entry) => entry.isIntersecting)) return;
    observer.disconnect();
    if (motion.matches) play(STATIC_SCENE);
  }, { threshold: 0.4 });
  observer.observe(section);
}
