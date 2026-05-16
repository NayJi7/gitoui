// Tiny ping counter for the curl install one-liner. install.sh fires
// `GET /ping` early so we can tally how many people are running the
// installer; `GET /count` returns a shields.io-shaped JSON the README
// badge consumes.

const KEY = "installs";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

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
      // KV writes aren't transactional — concurrent pings can lose
      // an increment. For an install counter the slop is acceptable
      // (we'd be off by a handful at most). For exact counts we'd
      // need a Durable Object, not worth the complexity here.
      await env.COUNTER.put(KEY, String(current + 1));
      return new Response("", { status: 204 });
    }

    if (request.method === "GET" && url.pathname === "/count") {
      // shields.io endpoint badge format — drop this URL into a
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
