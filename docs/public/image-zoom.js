(() => {
  const selector = 'main .sl-markdown-content img, main .content-panel img';
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  const easing = 'cubic-bezier(0.2, 0, 0, 1)';

  function imageLabel(img) {
    const alt = img.getAttribute('alt')?.trim();
    return alt ? `Zoom image: ${alt}` : 'Zoom image';
  }

  function imageCaption(img) {
    const block = img.closest('p') ?? img;
    const next = block.nextElementSibling;
    return next?.classList.contains('media-caption') ? next.textContent.trim() : '';
  }

  // The transform that places the enlarged image over the page thumbnail.
  function thumbnailTransform(thumbnail, image) {
    const from = thumbnail.getBoundingClientRect();
    const to = image.getBoundingClientRect();
    if (to.width === 0 || to.height === 0) return null;
    const x = from.left - to.left;
    const y = from.top - to.top;
    return `translate(${x}px, ${y}px) scale(${from.width / to.width}, ${from.height / to.height})`;
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

    const image = document.createElement('img');
    image.className = 'image-zoom-full';
    image.alt = '';

    const caption = document.createElement('p');
    caption.className = 'image-zoom-caption';

    frame.append(closeButton, image, caption);
    dialog.append(frame);
    document.body.append(dialog);

    const preview = { dialog, image, caption, thumbnail: null, pending: false, closing: false, opening: null };

    // Shrinks the image back onto its thumbnail before closing.
    function close() {
      if (preview.closing) return;
      const transform =
        !reducedMotion.matches && preview.thumbnail
          ? thumbnailTransform(preview.thumbnail, image)
          : null;
      if (!transform) {
        dialog.close();
        return;
      }
      preview.closing = true;
      dialog.classList.add('is-closing');
      // Holding the last frame keeps the image from snapping back to full
      // size before the dialog closes.
      const shrink = image.animate([{ transform: 'none' }, { transform }], {
        duration: 180,
        easing,
        fill: 'forwards',
      });
      shrink.finished.finally(() => {
        dialog.close();
        dialog.classList.remove('is-closing');
        shrink.cancel();
        preview.closing = false;
      });
    }

    closeButton.addEventListener('click', close);
    dialog.addEventListener('click', (event) => {
      // The second click of a quick double click on a thumbnail lands on
      // the backdrop; let the zoom finish before backdrop clicks close it.
      if (event.target === dialog && preview.opening?.playState !== 'running') close();
    });
    dialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      close();
    });

    preview.open = async (thumbnail) => {
      // Repeated clicks while the decode wait runs would restart the zoom.
      if (preview.pending || dialog.open) return;
      preview.pending = true;
      preview.thumbnail = thumbnail;
      image.src = thumbnail.currentSrc || thumbnail.src;
      image.alt = thumbnail.alt;
      caption.textContent = imageCaption(thumbnail);
      caption.hidden = caption.textContent === '';
      // The thumbnail already loaded this source; the timeout only guards
      // against decode never settling, as in background tabs.
      await Promise.race([image.decode().catch(() => {}), new Promise((r) => setTimeout(r, 150))]);
      preview.pending = false;
      dialog.showModal();
      if (reducedMotion.matches) return;
      const transform = thumbnailTransform(thumbnail, image);
      if (transform) {
        preview.opening = image.animate([{ transform }, { transform: 'none' }], {
          duration: 260,
          easing,
        });
      }
    };

    return preview;
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

      trigger.addEventListener('click', () => preview.open(img));
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initImageZoom, { once: true });
  } else {
    initImageZoom();
  }
})();
