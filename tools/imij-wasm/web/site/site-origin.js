// Page origin and data origin of the deployed site, filled in by the packager.
//
// Pages (site, benchmarks, Encoder Lab, explorer, Atlas, spec) have one
// canonical origin. Immutable data (tiles under /d/, previews under /p/,
// benchmark files under /benchmarks/) lives on a separate data origin. Pages
// resolve data paths against the data origin, never against wherever the page
// happens to be served. A page opened on the data origin (old copies of the
// site exist there) moves to the same section on the page origin, keeping the
// query and fragment. Preview and local origins are left alone.
//
// Unpackaged source (local development) keeps both origins equal to the
// current one.
(() => {
  const filled = value => (/^https?:\/\//.test(value) ? new URL(value).origin : null);
  const pageOrigin = filled('__PAGE_ORIGIN__');
  const dataOrigin = filled('__DATA_ORIGIN__') || location.origin;
  window.IMI_SITE = Object.freeze({ pageOrigin: pageOrigin || location.origin, dataOrigin });
  if (!pageOrigin || location.origin === pageOrigin || location.origin !== dataOrigin) return;
  let path = location.pathname;
  if (path === '/' || path === '/index.html' || path === '/gigapixel/index.html') path = '/gigapixel/';
  location.replace(pageOrigin + path + location.search + location.hash);
})();
