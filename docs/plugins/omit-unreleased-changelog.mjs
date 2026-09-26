import path from 'node:path';

export default function omitUnreleasedChangelog() {
  return (tree, file) => {
    if (path.basename(file.path).toLowerCase() !== 'changelog.md') return;

    const start = tree.children.findIndex(
      (node) =>
        node.type === 'heading' &&
        node.depth === 2 &&
        headingText(node).trim().toLowerCase() === 'unreleased',
    );
    if (start === -1) return;

    const nextSection = tree.children.findIndex(
      (node, index) => index > start && node.type === 'heading' && node.depth <= 2,
    );
    tree.children.splice(start, (nextSection === -1 ? tree.children.length : nextSection) - start);
  };
}

function headingText(node) {
  return node.children
    .map((child) => {
      if ('value' in child) return child.value;
      if ('children' in child) return headingText(child);
      return '';
    })
    .join('');
}
