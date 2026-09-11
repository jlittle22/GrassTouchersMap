// Bump when changing what/how we cache; caches with any other name are deleted on activate.
var cacheName = 'gtmap-pwa-v2';
var filesToCache = [
  './',
  './index.html',
  './turunmap.js',
  './turunmap_bg.wasm',
];

/* Cache the app shell, and take over from any older service worker right away */
self.addEventListener('install', function (e) {
  self.skipWaiting();
  e.waitUntil(
    caches.open(cacheName).then(function (cache) {
      return cache.addAll(filesToCache.map(function (file) {
        return new Request(file, { cache: 'no-cache' });
      }));
    })
  );
});

/* Drop caches from older versions (the old cache-first worker pinned stale builds forever) */
self.addEventListener('activate', function (e) {
  e.waitUntil(
    caches.keys().then(function (keys) {
      return Promise.all(
        keys.filter(function (key) { return key !== cacheName; })
          .map(function (key) { return caches.delete(key); })
      );
    }).then(function () {
      return self.clients.claim();
    })
  );
});

/* Network first so new deploys show up on the next load; serve the cache only when offline.
   cache: 'no-cache' revalidates with the server even if the browser's HTTP cache thinks its copy
   is fresh (we don't content-hash file names). Cache keys ignore the query string, since
   ?server=..&selections=.. varies per link. */
self.addEventListener('fetch', function (e) {
  var url = new URL(e.request.url);
  if (e.request.method !== 'GET' || url.origin !== self.location.origin) {
    return;
  }
  var key = url.origin + url.pathname;
  e.respondWith(
    fetch(e.request, { cache: 'no-cache' }).then(function (response) {
      if (response.ok) {
        var copy = response.clone();
        caches.open(cacheName).then(function (cache) {
          cache.put(key, copy);
        });
      }
      return response;
    }).catch(function () {
      return caches.match(key);
    })
  );
});
