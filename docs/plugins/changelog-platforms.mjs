import path from 'node:path';

const platforms = [
  { label: 'CLI & TUI', href: '/changelog/', matches: (file) => file === 'changelog.md' },
  { label: 'iPhone', href: '/changelog/ios/', matches: (file) => file === 'changelog/ios.md' },
  { label: 'Android', href: '/changelog/android/', matches: (file) => file === 'changelog/android.md' },
];

// Opens each changelog page with a switcher between the platforms' release notes.
export default function changelogPlatforms() {
  return (tree, file) => {
    const name = path.basename(file.path).toLowerCase();
    const parent = path.basename(path.dirname(file.path)).toLowerCase();
    const relative = parent === 'changelog' ? `changelog/${name}` : name;
    if (!platforms.some((platform) => platform.matches(relative))) return;

    const links = platforms
      .map((platform) => {
        const current = platform.matches(relative) ? ' aria-current="page"' : '';
        return `<a href="${platform.href}"${current}>${platform.label}</a>`;
      })
      .join('');
    tree.children.unshift({
      type: 'html',
      value: `<nav class="changelog-platforms not-content" aria-label="Changelog platform">${links}</nav>`,
    });
  };
}
