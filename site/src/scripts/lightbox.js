// Opens feature screenshots in a modal overlay without leaving the page.
const dialog = document.getElementById('lightbox');
if (dialog) {
  const img = dialog.querySelector('img');
  const cap = dialog.querySelector('.lightbox-cap');
  let opener = null;

  document.querySelectorAll('.shot-btn').forEach((btn) => {
    btn.addEventListener('click', () => {
      const thumb = btn.querySelector('img');
      opener = btn;
      img.src = btn.dataset.full;
      img.alt = thumb.alt;
      cap.textContent = thumb.alt;
      dialog.showModal();
    });
  });

  const close = () => dialog.close();
  dialog.querySelector('.lightbox-close').addEventListener('click', close);
  img.addEventListener('click', close);
  // a click on the backdrop lands on the dialog element itself
  dialog.addEventListener('click', (e) => { if (e.target === dialog) close(); });
  dialog.addEventListener('close', () => {
    img.removeAttribute('src');
    opener?.focus();
  });
}
