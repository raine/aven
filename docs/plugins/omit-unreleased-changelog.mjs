import path from 'node:path';

const releaseHeading = /\(\d{4}-\d{2}-\d{2}\)\s*$/;

// Keeps only released versions on changelog pages: a version section is shown
// once its heading carries a release date, such as "v0.1.49 (2026-10-13)".
export default function omitUnreleasedChangelog() {
  return (tree, file) => {
    if (!isChangelog(file.path)) return;

    const children = [];
    let omitting = false;
    for (const node of tree.children) {
      if (node.type === 'heading' && node.depth <= 2) {
        omitting = node.depth === 2 && !releaseHeading.test(headingText(node));
      }
      if (!omitting) children.push(node);
    }

    if (!children.some((node) => node.type === 'heading' && node.depth === 2)) {
      children.push({
        type: 'paragraph',
        children: [{ type: 'text', value: 'No public release yet.' }],
      });
    }
    tree.children = children;
  };
}

function isChangelog(filePath) {
  return (
    path.basename(filePath).toLowerCase() === 'changelog.md' ||
    path.basename(path.dirname(filePath)).toLowerCase() === 'changelog'
  );
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
