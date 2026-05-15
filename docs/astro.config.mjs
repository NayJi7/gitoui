// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// Site URL = the GitHub Pages URL the workflow deploys to.
// `base` matches the repo name so all absolute paths in the built
// HTML resolve under /gitoui/. Set GITOUI_DOCS_BASE='' locally to
// preview at the root (e.g. `npm run dev`).
const base = process.env.GITOUI_DOCS_BASE ?? "/gitoui";

// https://astro.build/config
export default defineConfig({
  site: "https://nayji7.github.io",
  base,
  trailingSlash: "ignore",
  integrations: [
    starlight({
      title: "gitoui",
      description:
        "Terminal UI git client — rich commit graph, PRs, Issues, and inline image-protocol rendering.",
      logo: {
        src: "./src/assets/logo-mark.svg",
        replacesTitle: false,
      },
      favicon: "/favicon.svg",
      social: {
        github: "https://github.com/NayJi7/gitoui",
      },
      editLink: {
        baseUrl: "https://github.com/NayJi7/gitoui/edit/master/docs/",
      },
      customCss: ["./src/styles/theme.css"],
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
          ],
        },
        {
          label: "Features",
          collapsed: false,
          items: [
            { label: "Commit graph", link: "/features/commit-graph/" },
            { label: "Diff & blame", link: "/features/diff-blame/" },
            { label: "Uncommitted & staging", link: "/features/uncommitted/" },
            { label: "Interactive rebase", link: "/features/interactive-rebase/" },
            { label: "Conflict editor", link: "/features/conflict-editor/" },
            { label: "Pull requests", link: "/features/pull-requests/" },
            { label: "Issues", link: "/features/issues/" },
            { label: "User commands", link: "/features/user-command/" },
          ],
        },
        {
          label: "FAQ",
          link: "/faq/",
        },
      ],
    }),
  ],
});
