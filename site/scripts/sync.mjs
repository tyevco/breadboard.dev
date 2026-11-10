// Copies the repository's design records into the site at build time, so
// they are rendered without being duplicated in git:
//
//   docs/adr/*.md       -> records/adr/
//   spec/*.md           -> records/spec/
//   docs/*.md           -> records/plan/
//
// The copies are git-ignored (site/.gitignore). Each one gets a short header
// that names its source file on GitHub. The sources are never modified.
//
// Links: a relative link between two copied files keeps working because the
// copies keep their directory structure. Any other relative link (to code,
// to a file outside the copied sets) is rewritten to a GitHub blob URL, so
// VitePress's dead-link check stays meaningful for the hand-written pages.
import { readdirSync, readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs'
import { dirname, join, posix, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const site = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repo = resolve(site, '..')
const BLOB = 'https://github.com/tyevco/breadboard.dev/blob/main/'

const sets = [
  { from: 'docs/adr', to: 'records/adr' },
  { from: 'spec', to: 'records/spec' },
  { from: 'docs', to: 'records/plan' },
]

// Source path (repo-relative, posix) -> copy path (site-relative, posix).
const copies = new Map()
for (const { from, to } of sets) {
  for (const name of readdirSync(join(repo, from)).sort()) {
    if (name.endsWith('.md')) copies.set(`${from}/${name}`, `${to}/${name}`)
  }
}

function rewriteLink(target, srcRel, dstRel) {
  if (/^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('#') || target.startsWith('/')) {
    return target
  }
  const [path, hash = ''] = target.split(/(?=#)/)
  const resolvedSrc = posix.normalize(posix.join(posix.dirname(srcRel), path))
  const copy = copies.get(resolvedSrc)
  if (copy) {
    return posix.relative(posix.dirname(dstRel), copy) + hash
  }
  return BLOB + resolvedSrc + hash
}

function transform(text, srcRel, dstRel) {
  const out = []
  let fence = null
  for (const line of text.split('\n')) {
    const m = line.match(/^\s*(`{3,}|~{3,})/)
    if (fence) {
      if (m && m[1][0] === fence[0] && m[1].length >= fence.length) fence = null
      out.push(line)
      continue
    }
    if (m) {
      fence = m[1]
      out.push(line)
      continue
    }
    // Markdown links outside fenced code: [text](target)
    out.push(line.replace(/\]\(([^)\s]+)\)/g, (_, t) => `](${rewriteLink(t, srcRel, dstRel)})`))
  }
  let body = out.join('\n')
  // Vue would treat {{ ... }} in prose as an interpolation; render such a
  // file verbatim instead (none of today's sources needs it).
  if (body.includes('{{')) body = `::: v-pre\n${body}\n:::\n`
  return body
}

for (const { to } of sets) rmSync(join(site, to), { recursive: true, force: true })

for (const [srcRel, dstRel] of copies) {
  const text = readFileSync(join(repo, srcRel), 'utf8')
  const lines = text.split('\n')
  // Keep the source's H1 as the page title; put the provenance note under it.
  const h1 = lines[0].startsWith('# ') ? lines.shift() : `# ${srcRel}`
  const note =
    `::: info Rendered from the repository\n` +
    `This page is a copy of [\`${srcRel}\`](${BLOB}${srcRel}), made when the site was built. ` +
    `The file in the repository is the authority; paths in it are relative to the repository root.\n` +
    `:::\n`
  const page = `${h1}\n\n${note}\n${transform(lines.join('\n'), srcRel, dstRel)}`
  const dst = join(site, dstRel)
  mkdirSync(dirname(dst), { recursive: true })
  writeFileSync(dst, page)
}

console.log(`sync: copied ${copies.size} files into ${sets.map((s) => s.to).join(', ')}`)
