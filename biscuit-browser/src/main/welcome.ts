// The Biscuit Browser welcome / new-tab page. Served as a self-contained
// `data:` URL (no bundled file, no network), styled to match the app's clean
// white theme. Shown instead of a search engine when a tab opens with no URL.

const HTML = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Biscuit Browser</title>
<style>
  :root { color-scheme: light; }
  * { box-sizing: border-box; }
  html, body { height: 100%; margin: 0; }
  body {
    display: flex;
    align-items: center;
    justify-content: center;
    background: #ffffff;
    color: #1d1d1f;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'SF Pro Display', 'Segoe UI', Roboto, Helvetica, Arial, sans-serif;
    -webkit-font-smoothing: antialiased;
    letter-spacing: -0.006em;
  }
  .wrap { width: 100%; max-width: 600px; padding: 40px 28px; text-align: center; }
  .logo { font-size: 68px; line-height: 1; margin-bottom: 16px; }
  h1 { font-size: 32px; font-weight: 600; letter-spacing: -0.02em; margin: 0 0 10px; }
  .tag { font-size: 16px; line-height: 1.55; color: #6e6e76; margin: 0 auto 30px; max-width: 460px; }
  .cards { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; text-align: left; }
  .card { border: 1px solid #e4e4ea; border-radius: 14px; padding: 15px 16px; background: #fafafa; }
  .card h3 { margin: 0 0 5px; font-size: 13.5px; font-weight: 600; }
  .card p { margin: 0; font-size: 12.5px; line-height: 1.5; color: #6e6e76; }
  .hint { margin-top: 26px; font-size: 12.5px; color: #9a9aa6; line-height: 1.6; }
  .kbd { font-family: ui-monospace, Menlo, Consolas, monospace; background: #f1f1f4; border: 1px solid #e4e4ea; border-radius: 6px; padding: 1px 7px; font-size: 12px; color: #1d1d1f; }
  @media (max-width: 460px) { .cards { grid-template-columns: 1fr; } }
</style>
</head>
<body>
  <main class="wrap">
    <div class="logo">🍪</div>
    <h1>Biscuit Browser</h1>
    <p class="tag">An AI-native browser. Tell the agent what to do in the side panel — it reads the page as an Agent View and acts through a permission gate, so you stay in control.</p>
    <div class="cards">
      <div class="card"><h3>💬 Ask the agent</h3><p>Type a task in Chat, e.g. &ldquo;find the latest stable Rust version.&rdquo;</p></div>
      <div class="card"><h3>🔒 Stay in control</h3><p>Pick a mode (Safe / Assisted / Auto). Approve risky actions right in the chat.</p></div>
      <div class="card"><h3>📋 Task plan</h3><p>A plain-text plan is generated from your prompt before anything runs.</p></div>
      <div class="card"><h3>🧩 Agent-ready</h3><p>Exposes an MCP server so Biscuit Code &amp; other AI agents can drive it.</p></div>
    </div>
    <p class="hint">Type a URL or search in the bar above, or just ask in the side panel.<br />No API key yet? Try <span class="kbd">▶ Run keyless demo</span> in Chat.</p>
  </main>
</body>
</html>`

/** The welcome page as a loadable data: URL. */
export const WELCOME_URL = `data:text/html;charset=utf-8,${encodeURIComponent(HTML)}`

/** True if a URL is the welcome page (so the UI can show a friendly address). */
export function isWelcomeUrl(url: string): boolean {
  return url.startsWith('data:text/html') && url.includes('Biscuit%20Browser')
}
