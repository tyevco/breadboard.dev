import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig, type DefaultTheme } from 'vitepress'

const REPO_URL = 'https://github.com/tyevco/breadboard.dev'
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')

// Sidebar entries for the design records that scripts/sync.mjs copies into
// records/ (see that script). The text is each file's own H1.
function recordItems(srcDir: string, route: string): DefaultTheme.SidebarItem[] {
  return readdirSync(join(repoRoot, srcDir))
    .filter((f) => f.endsWith('.md'))
    .sort()
    .map((f) => {
      const first = readFileSync(join(repoRoot, srcDir, f), 'utf8').split('\n', 1)[0]
      const text = first.replace(/^#\s+/, '').replace(/`/g, '')
      return { text, link: `${route}/${f.replace(/\.md$/, '')}` }
    })
}

export default defineConfig({
  title: 'Breadboard',
  description:
    'An open hardware simulator: real firmware on a real KiCad board, Renode and ngspice co-simulated under one kernel-owned clock.',
  base: '/breadboard.dev/',
  lang: 'en-US',
  cleanUrls: true,
  lastUpdated: false,
  markdown: {
    // The design records copied from docs/ and spec/ write placeholders such
    // as <board.cir> and <instance> in prose. With raw HTML enabled, Vue
    // would parse those as unclosed elements; no page on this site needs
    // raw HTML, so it is rendered as text.
    html: false,
  },
  head: [['meta', { name: 'theme-color', content: '#3c8772' }]],
  themeConfig: {
    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '/guide/' },
      { text: 'Concepts', link: '/concepts/cosim', activeMatch: '/concepts/' },
      { text: 'Reference', link: '/reference/cli', activeMatch: '/reference/' },
      { text: 'Design records', link: '/records/', activeMatch: '/records/' },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Guide',
          items: [
            { text: 'Introduction', link: '/guide/introduction' },
            { text: 'Quickstart', link: '/guide/quickstart' },
            { text: 'The E1 demo', link: '/guide/demo' },
            { text: 'Installing and setup modes', link: '/guide/setup' },
            { text: 'Running the tests', link: '/guide/testing' },
            { text: 'Project layout', link: '/guide/layout' },
            { text: 'Status and roadmap', link: '/guide/roadmap' },
          ],
        },
      ],
      '/concepts/': [
        {
          text: 'Concepts',
          items: [
            { text: 'Co-simulation and time grants', link: '/concepts/cosim' },
            { text: 'Determinism', link: '/concepts/determinism' },
            { text: 'Fidelity and honesty', link: '/concepts/fidelity' },
            { text: 'Performance', link: '/concepts/performance' },
          ],
        },
      ],
      '/reference/': [
        {
          text: 'Reference',
          items: [
            { text: 'CLI (bb)', link: '/reference/cli' },
            { text: 'Projects and benches', link: '/reference/project' },
            { text: 'Component packages', link: '/reference/packages' },
            { text: 'Pin models', link: '/reference/pin-models' },
            { text: 'Outputs and the event stream', link: '/reference/outputs' },
          ],
        },
      ],
      '/records/': [
        { text: 'Design records', items: [{ text: 'Overview', link: '/records/' }] },
        { text: 'Architecture decisions', collapsed: false, items: recordItems('docs/adr', '/records/adr') },
        { text: 'Specifications', collapsed: false, items: recordItems('spec', '/records/spec') },
        { text: 'Plans and checklists', collapsed: false, items: recordItems('docs', '/records/plan') },
      ],
    },
    socialLinks: [{ icon: 'github', link: REPO_URL }],
    editLink: {
      // VitePress serializes this function into the client bundle, so it
      // must not refer to anything outside its own body. The copied design
      // records link to their source file, not to the git-ignored copy.
      pattern: ({ filePath }) => {
        const repo = 'https://github.com/tyevco/breadboard.dev/edit/main/'
        const sources: [string, string][] = [
          ['records/adr/', 'docs/adr/'],
          ['records/spec/', 'spec/'],
          ['records/plan/', 'docs/'],
        ]
        for (const [copy, src] of sources) {
          if (filePath.startsWith(copy)) return repo + src + filePath.slice(copy.length)
        }
        return repo + 'site/' + filePath
      },
      text: 'Edit this page on GitHub',
    },
    search: { provider: 'local' },
    outline: { level: [2, 3] },
    footer: {
      message: 'Released under the Apache-2.0 License.',
    },
  },
})
