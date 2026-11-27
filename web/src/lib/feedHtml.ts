// POD6: a feed's description, made safe to show. Podcast feeds carry
// show notes as HTML (POD5 reading 12), written by whoever publishes
// the feed, so it is never put into a page as it comes. Instead it is
// read into an inert document (DOMParser: no scripts run, nothing is
// fetched) and written out again from an allowlist:
//
// - kept: paragraphs, line breaks, lists, quotes, bold / italic /
//   underline, code, and headings (as bold paragraphs);
// - links: only http(s) and mailto addresses, opened in a new tab with
//   no referrer and no opener;
// - dropped with everything inside them: scripts, styles, frames,
//   forms, media and images — an image would hot-link the publisher
//   (POD4 reading 11), which hands them the listener's address;
// - any other element is unwrapped: its text stays, the element goes;
// - no attribute survives except a link's checked `href`.
//
// A description with no tags at all is plain text: it is escaped, and
// its line breaks are kept.

const KEEP = new Set([
  'p',
  'br',
  'b',
  'strong',
  'i',
  'em',
  'u',
  'ul',
  'ol',
  'li',
  'blockquote',
  'code',
  'pre',
]);

const HEADINGS = new Set(['h1', 'h2', 'h3', 'h4', 'h5', 'h6']);

const DROP = new Set([
  'script',
  'style',
  'iframe',
  'frame',
  'frameset',
  'object',
  'embed',
  'applet',
  'img',
  'picture',
  'source',
  'video',
  'audio',
  'track',
  'canvas',
  'svg',
  'math',
  'form',
  'input',
  'button',
  'select',
  'textarea',
  'template',
  'noscript',
  'head',
  'title',
  'link',
  'meta',
  'base',
]);

function escapeText(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function escapeAttr(s: string): string {
  return escapeText(s).replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

/** A link's address if it may be followed: absolute http(s) or mailto. */
export function safeHref(raw: string | null): string | null {
  if (!raw) return null;
  try {
    const url = new URL(raw.trim());
    if (url.protocol === 'http:' || url.protocol === 'https:' || url.protocol === 'mailto:') {
      return url.href;
    }
  } catch {
    // Not an absolute address: relative links mean nothing off the feed.
  }
  return null;
}

function write(node: Node, out: string[]): void {
  if (node.nodeType === Node.TEXT_NODE) {
    out.push(escapeText(node.textContent ?? ''));
    return;
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return; // comments, etc.
  const el = node as Element;
  const tag = el.tagName.toLowerCase();
  if (DROP.has(tag)) return;
  const children = () => el.childNodes.forEach((c) => write(c, out));
  if (tag === 'a') {
    const href = safeHref(el.getAttribute('href'));
    if (!href) {
      children();
      return;
    }
    out.push(`<a href="${escapeAttr(href)}" target="_blank" rel="noopener noreferrer nofollow">`);
    children();
    out.push('</a>');
    return;
  }
  if (HEADINGS.has(tag)) {
    out.push('<p><strong>');
    children();
    out.push('</strong></p>');
    return;
  }
  if (KEEP.has(tag)) {
    if (tag === 'br') {
      out.push('<br />');
      return;
    }
    out.push(`<${tag}>`);
    children();
    out.push(`</${tag}>`);
    return;
  }
  children();
}

/** Does this look like HTML rather than plain text? */
function hasMarkup(s: string): boolean {
  return /<\/?[a-z][^>]*>/i.test(s) || /&(#\d+|#x[0-9a-f]+|[a-z]+);/i.test(s);
}

/** The description as safe HTML for `{@html}`; '' for nothing. */
export function feedHtml(source: string | null | undefined): string {
  const text = (source ?? '').trim();
  if (!text) return '';
  if (!hasMarkup(text)) {
    return text
      .split(/\n{2,}/)
      .map((para) => `<p>${escapeText(para).replace(/\n/g, '<br />')}</p>`)
      .join('');
  }
  const doc = new DOMParser().parseFromString(text, 'text/html');
  const out: string[] = [];
  doc.body.childNodes.forEach((c) => write(c, out));
  return out.join('');
}

/** The description as one line of plain text (for a row's summary). */
export function feedText(source: string | null | undefined): string {
  const text = (source ?? '').trim();
  if (!text) return '';
  if (!hasMarkup(text)) return text.replace(/\s+/g, ' ');
  const doc = new DOMParser().parseFromString(text, 'text/html');
  doc.querySelectorAll([...DROP].join(',')).forEach((n) => n.remove());
  return (doc.body.textContent ?? '').replace(/\s+/g, ' ').trim();
}
