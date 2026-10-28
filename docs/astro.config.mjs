// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// Site URL = the custom domain set in the repo's GitHub Pages
// settings. The site is served from the domain root, so `base` is ""
// (the old https://nayji7.github.io/gitoui/* URLs are redirected here
// by GitHub). `GITOUI_DOCS_BASE` still wins when set explicitly, e.g.
// to preview a build under a sub-path.
const base = process.env.GITOUI_DOCS_BASE ?? "";
const site = "https://gitoui.nayji7.dev";

// Social preview card (1200x630) shown by Google, GitHub, Slack,
// X, etc. Crawlers need an absolute URL, so it's built from site+base.
const ogImage = new URL(`${base}/og.png`, site).href;

// https://astro.build/config
export default defineConfig({
  site,
  base,
  trailingSlash: "ignore",
  integrations: [
    starlight({
      title: "gitoui",
      description: "Say oui to the smoothest git terminal youser experience.",
      // Use our custom 404 at src/pages/404.astro instead of Starlight's
      // generated one - fixes the "route /404 defined twice" warning.
      disable404Route: true,
      // Show the page's last commit date in the doc footer. Starlight
      // reads it from `git log -1 --format=%aI <file>` at build time.
      lastUpdated: true,
      logo: {
        src: "./src/assets/logo-mark.svg",
        replacesTitle: false,
      },
      favicon: "/favicon.svg",
      head: [
        { tag: "meta", attrs: { property: "og:image", content: ogImage } },
        { tag: "meta", attrs: { property: "og:image:width", content: "1200" } },
        { tag: "meta", attrs: { property: "og:image:height", content: "630" } },
        { tag: "meta", attrs: { property: "og:image:alt", content: "gitoui, a terminal UI for git" } },
        { tag: "meta", attrs: { name: "twitter:image", content: ogImage } },
        // Raster fallback next to favicon.svg: Google Search shows the
        // site icon from the home page and prefers a 48px-multiple PNG.
        {
          tag: "link",
          attrs: { rel: "icon", type: "image/png", sizes: "96x96", href: `${base}/favicon-96.png` },
        },
        {
          tag: "link",
          attrs: { rel: "apple-touch-icon", sizes: "180x180", href: `${base}/apple-touch-icon.png` },
        },
      ],
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/NayJi7/gitoui" },
      ],
      editLink: {
        baseUrl: "https://github.com/NayJi7/gitoui/edit/master/docs/",
      },
      customCss: ["./src/styles/theme.css"],
      components: {
        SiteTitle: "./src/components/SiteTitle.astro",
        SocialIcons: "./src/components/SocialIcons.astro",
        ThemeSelect: "./src/components/ThemeSelect.astro",
        Hero: "./src/components/Hero.astro",
        Footer: "./src/components/Footer.astro",
      },
      sidebar: [
        {
          label: "Introduction",
          link: "/introduction/",
        },
        {
          label: "Getting started",
          collapsed: false,
          items: [
            { label: "Requirements", link: "/getting-started/requirements/" },
            { label: "Installation", link: "/getting-started/installation/" },
            { label: "Basic usage", link: "/getting-started/basic-usage/" },
            { label: "CLI options", link: "/getting-started/cli/" },
            { label: "Terminal compatibility", link: "/getting-started/compatibility/" },
          ],
        },
        {
          label: "Configuration",
          collapsed: false,
          items: [
            { label: "Config file", link: "/configurations/config-file-format/" },
            { label: "Themes", link: "/configurations/themes/" },
          ],
        },
        {
          label: "Keybindings",
          collapsed: false,
          items: [
            { label: "Overview", link: "/keybindings/" },
            { label: "Custom keybindings", link: "/keybindings/custom/" },
            { label: "User commands", link: "/keybindings/user-command/" },
          ],
        },
        {
          label: "Features",
          collapsed: false,
          items: [
            { label: "Commit graph", link: "/features/commit-graph/" },
            { label: "Diff, blame & history", link: "/features/diff-blame/" },
            { label: "Uncommitted & staging", link: "/features/uncommitted/" },
            { label: "Interactive rebase", link: "/features/interactive-rebase/" },
            { label: "Conflict editor", link: "/features/conflict-editor/" },
            { label: "Pull requests", link: "/features/pull-requests/" },
            { label: "Issues", link: "/features/issues/" },
          ],
        },
        {
          label: "FAQ",
          link: "/faq/",
        },
        {
          label: "Brand",
          link: "/brand",
        },
        {
          label: "License",
          link: "/license/",
        },
      ],
    }),
  ],
});
