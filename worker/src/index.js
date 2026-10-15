// Tiny ping counter for the curl install one-liner. install.sh fires
// `GET /ping` early so we can tally how many people are running the
// installer; `GET /count` returns a shields.io-shaped JSON the README
// badge consumes.

const KEY = "installs";

// Homebrew publishes its own install analytics for every public tap
// (https://docs.brew.sh/Analytics). `install-on-request` counts the
// installs a user explicitly asked for (not dependency pulls), straight
// from `brew install`: no mixing with curl or manual downloads. Users
// who turned analytics off are not counted, so this is a lower bound.
const BREW_URL = "https://formulae.brew.sh/api/analytics/install-on-request/365d.json";
const BREW_FORMULA = "nayji7/tap/gitoui";
const BREW_KEY = "brew";
const BREW_REFRESH_MS = 12 * 60 * 60 * 1000;
// The feed is ~4 MB. Match the one record we need instead of parsing it.
const BREW_RECORD = new RegExp(
  `"formula":"${BREW_FORMULA}","count":"([\\d,]+)"`,
);

// Returns the stored cumulative brew count, refreshing it at most every
// BREW_REFRESH_MS. The feed is a rolling 365-day window, so the stored
// value is a high-water mark: it only goes up, and a failed or empty
// fetch never lowers it. Installs started 2026-10-12, so nothing drops
// out of the window before 2027-10-12; past that date the stored value
// has to be accumulated from periodic snapshots instead.
export async function brewCount(env, now = Date.now(), fetchFn = fetch) {
  const stored = JSON.parse((await env.COUNTER.get(BREW_KEY)) || "{}");
  if (stored.checked && now - stored.checked < BREW_REFRESH_MS) {
    return stored.count || 0;
  }
  let count = stored.count || 0;
  try {
    const res = await fetchFn(BREW_URL, {
      cf: { cacheTtl: 6 * 60 * 60, cacheEverything: true },
    });
    if (res.ok) {
      const m = BREW_RECORD.exec(await res.text());
      if (m) count = Math.max(count, parseInt(m[1].replace(/,/g, ""), 10));
    }
  } catch {
    // keep the stored value
  }
  await env.COUNTER.put(BREW_KEY, JSON.stringify({ count, checked: now }));
  return count;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (request.method === "GET" && url.pathname === "/brew") {
      const count = await brewCount(env);
      return Response.json(
        {
          schemaVersion: 1,
          label: "brew installs",
          message: count.toLocaleString("en-US"),
          color: "F05133",
          cacheSeconds: 3600,
        },
        {
          headers: {
            "Cache-Control": "public, max-age=3600",
            "Access-Control-Allow-Origin": "*",
          },
        },
      );
    }

    if (request.method === "GET" && url.pathname === "/ping") {
      // Only count requests that look like our installer. Browsers
      // and naive scrapers send `Mozilla/...`; we reject those to
      // keep the number meaningful. install.sh runs via `curl|bash`
      // so the UA starts with `curl/` (or `Wget/` if a user manually
      // swaps the fetcher).
      const ua = request.headers.get("user-agent") || "";
      if (!ua.startsWith("curl/") && !ua.startsWith("Wget/")) {
        return new Response("", { status: 204 });
      }
      const current = parseInt((await env.COUNTER.get(KEY)) || "0", 10);
      // KV writes aren't transactional - concurrent pings can lose
      // an increment. For an install counter the slop is acceptable
      // (we'd be off by a handful at most). For exact counts we'd
      // need a Durable Object, not worth the complexity here.
      await env.COUNTER.put(KEY, String(current + 1));
      return new Response("", { status: 204 });
    }

    if (request.method === "GET" && url.pathname === "/count") {
      // shields.io endpoint badge format - drop this URL into a
      // `https://img.shields.io/endpoint?url=...` and it renders.
      const count = parseInt((await env.COUNTER.get(KEY)) || "0", 10);
      return Response.json(
        {
          schemaVersion: 1,
          label: "curl installs",
          message: count.toLocaleString("en-US"),
          color: "F05133",
          cacheSeconds: 300,
        },
        {
          headers: {
            "Cache-Control": "public, max-age=300",
            "Access-Control-Allow-Origin": "*",
          },
        },
      );
    }

    return new Response("Not found", { status: 404 });
  },
};
