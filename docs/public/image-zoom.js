(() => {
  const selector = 'main .sl-markdown-content img, main .content-panel img';

  function imageLabel(img) {
    const alt = img.getAttribute('alt')?.trim();
    return alt ? `Zoom image: ${alt}` : 'Zoom image';
  }

  function imageCaption(img) {
    const block = img.closest('p') ?? img;
    const next = block.nextElementSibling;
    return next?.classList.contains('media-caption') ? next.textContent.trim() : '';
  }

  function closeOnBackdrop(event) {
    if (event.target === event.currentTarget) {
      event.currentTarget.close();
    }
  }

  function createDialog() {
    const dialog = document.createElement('dialog');
    dialog.className = 'image-zoom-dialog';
    dialog.setAttribute('aria-label', 'Image preview');

    const frame = document.createElement('div');
    frame.className = 'image-zoom-frame';

    const closeButton = document.createElement('button');
    closeButton.type = 'button';
    closeButton.className = 'image-zoom-close';
    closeButton.setAttribute('aria-label', 'Close image preview');
    closeButton.textContent = '×';
    closeButton.addEventListener('click', () => dialog.close());

    const image = document.createElement('img');
    image.className = 'image-zoom-full';
    image.alt = '';

    const caption = document.createElement('p');
    caption.className = 'image-zoom-caption';

    frame.append(closeButton, image, caption);
    dialog.append(frame);
    dialog.addEventListener('click', closeOnBackdrop);
    document.body.append(dialog);

    return { dialog, image, caption };
  }

  function initImageZoom() {
    const images = Array.from(document.querySelectorAll(selector));
    if (images.length === 0) return;

    const preview = createDialog();

    for (const img of images) {
      if (img.closest('a, button, .image-zoom-trigger')) continue;

      const trigger = document.createElement('button');
      trigger.type = 'button';
      trigger.className = 'image-zoom-trigger';
      trigger.setAttribute('aria-label', imageLabel(img));

      img.parentNode?.insertBefore(trigger, img);
      trigger.append(img);

      trigger.addEventListener('click', () => {
        preview.image.src = img.currentSrc || img.src;
        preview.image.alt = img.alt;
        preview.caption.textContent = imageCaption(img);
        preview.caption.hidden = preview.caption.textContent === '';
        preview.dialog.showModal();
      });
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initImageZoom, { once: true });
  } else {
    initImageZoom();
  }
})();
