// Public browser client for inspection and exact IQA pair input.
export class ArchiveClient {
  #worker = new Worker(new URL('./archive-worker.js', import.meta.url), {type: 'module'});
  #pending = new Map();
  #id = 0;

  constructor() {
    this.#worker.onmessage = ({data: {id, result, error}}) => {
      const pending = this.#pending.get(id);
      if (!pending) return;
      this.#pending.delete(id);
      if (error) pending.reject(Error(error));
      else pending.resolve(result);
    };
    this.#worker.onerror = error => this.#reject(Error(error.message || 'Archive worker failed.'));
  }

  #call(method, ...args) {
    return new Promise((resolve, reject) => {
      const id = ++this.#id;
      this.#pending.set(id, {resolve, reject});
      this.#worker.postMessage({id, method, args});
    });
  }

  #reject(error) {
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }

  open(urlOrFile) { return this.#call('open', urlOrFile); }
  list(start, count) { return this.#call('list', start, count); }
  pairs(index) { return this.#call('pairs', index); }
  previews(keys, thumbnails = false) { return this.#call('previews', keys, thumbnails); }
  entry(key) { return this.#call('entry', key); }
  exact(asset) { return this.#call('exact', asset); }
  stats() { return this.#call('stats'); }

  fetchPair(pair) { return this.#call('pair', pair); }

  close() {
    this.#worker.terminate();
    this.#reject(new DOMException('Archive closed.', 'AbortError'));
  }
}
