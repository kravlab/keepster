import { afterEach, describe, expect, it, vi } from 'vitest'
import { FolderBrowser, parentDirectory } from './folders.svelte'
import type { DirectoryListing, Subdirectory } from './wire'

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

const problem = (status: number, code: string, detail: string) =>
  json({ title: 'Refused', status, detail, code }, status, 'application/problem+json')

const folder = (path: string, over: Partial<Subdirectory> = {}): Subdirectory => ({
  name: path.slice(path.lastIndexOf('/') + 1),
  path,
  hidden: false,
  readable: true,
  ...over,
})

/** A listing of `path` holding `directories`, with `/home` above it unless told otherwise. */
const listing = (path: string, directories: Subdirectory[], parent: string | null = '/home'): DirectoryListing => ({
  path,
  parent,
  directories,
})

/** Answers each request in turn with the next of `answers`, recording the URLs asked for. */
const serve = (...answers: (() => Response)[]) => {
  const asked: string[] = []
  vi.stubGlobal('fetch', (url: string) => {
    asked.push(url)
    const next = answers.shift()
    if (!next) throw new Error(`no answer left for ${url}`)
    return Promise.resolve(next())
  })
  return asked
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the folder browser', () => {
  it('lists the home directory when it opens', async () => {
    const asked = serve(() => json(listing('/home/u', [folder('/home/u/src'), folder('/home/u/notes')])))
    const browser = new FolderBrowser(String)

    await browser.open()

    expect(asked).toEqual(['/v1/fs'])
    expect(browser.path).toBe('/home/u')
    expect(browser.directories.map((d) => d.name)).toEqual(['src', 'notes'])
  })

  it('goes into a subdirectory and back up to the parent', async () => {
    const asked = serve(
      () => json(listing('/home/u', [folder('/home/u/my src')])),
      () => json(listing('/home/u/my src', [], '/home/u')),
      () => json(listing('/home/u', [folder('/home/u/my src')])),
    )
    const browser = new FolderBrowser(String)
    await browser.open()

    await browser.enter(folder('/home/u/my src'))
    expect(browser.path).toBe('/home/u/my src')
    await browser.up()

    expect(asked).toEqual(['/v1/fs', '/v1/fs?path=/home/u/my%20src', '/v1/fs?path=/home/u'])
    expect(browser.path).toBe('/home/u')
  })

  it('leaves hidden folders out until asked for them', async () => {
    serve(() => json(listing('/home/u', [folder('/home/u/.config', { hidden: true }), folder('/home/u/src')])))
    const browser = new FolderBrowser(String)
    await browser.open()

    expect(browser.directories.map((d) => d.name)).toEqual(['src'])
    browser.showHidden = true
    expect(browser.directories.map((d) => d.name)).toEqual(['.config', 'src'])
  })

  it('marks a folder that cannot be listed and does not go into it', async () => {
    const asked = serve(() => json(listing('/home/u', [folder('/home/u/locked', { readable: false })])))
    const browser = new FolderBrowser(String)
    await browser.open()

    expect(browser.directories).toEqual([expect.objectContaining({ name: 'locked', readable: false })])
    await browser.enter(folder('/home/u/locked', { readable: false }))

    expect(asked).toEqual(['/v1/fs'])
    expect(browser.path).toBe('/home/u')
  })

  it('lists an absolute path typed or pasted in', async () => {
    const asked = serve(
      () => json(listing('/home/u', [])),
      () => json(listing('/srv/data', [folder('/srv/data/logs')], '/srv')),
    )
    const browser = new FolderBrowser(String)
    await browser.open()

    await browser.open('/srv/data')

    expect(asked).toEqual(['/v1/fs', '/v1/fs?path=/srv/data'])
    expect(browser.path).toBe('/srv/data')
    expect(browser.directories.map((d) => d.name)).toEqual(['logs'])
  })

  it.each([
    [404, 'path_not_found', '/home/u/gone: No such file or directory'],
    [409, 'not_a_directory', '/home/u/notes.txt is not a directory'],
    [409, 'directory_unreadable', '/root cannot be listed: Permission denied'],
    [400, 'path_invalid', 'the path must be absolute'],
  ])('says why a place cannot be listed (%i %s) and stays where it was', async (status, code, detail) => {
    serve(
      () => json(listing('/home/u', [folder('/home/u/src')])),
      () => problem(status, code, detail),
    )
    const browser = new FolderBrowser((e) => (e instanceof Error ? e.message : String(e)))
    await browser.open()

    await browser.open('/somewhere')

    expect(browser.error).toBe(detail)
    expect(browser.path).toBe('/home/u')
    expect(browser.directories.map((d) => d.name)).toEqual(['src'])
  })

  it('forgets a refusal once a listing succeeds', async () => {
    serve(
      () => problem(404, 'path_not_found', 'nothing there'),
      () => json(listing('/home/u', [])),
    )
    const browser = new FolderBrowser(String)
    await browser.open('/gone')
    expect(browser.error).not.toBe('')

    await browser.open()

    expect(browser.error).toBe('')
  })

  it('keeps the newest listing when an earlier one answers last', async () => {
    const answers: ((body: DirectoryListing) => void)[] = []
    vi.stubGlobal('fetch', () => new Promise<Response>((resolve) => answers.push((body) => resolve(json(body)))))
    const browser = new FolderBrowser(String)

    const first = browser.open('/home/u/a')
    const second = browser.open('/home/u/b')
    answers[1]?.(listing('/home/u/b', []))
    answers[0]?.(listing('/home/u/a', []))
    await Promise.all([first, second])

    expect(browser.path).toBe('/home/u/b')
  })

  describe('the path field and the directory it gives to add', () => {
    it('shows the directory listed, and gives it as it is without asking again', async () => {
      const asked = serve(() => json(listing('/home/u', [])))
      const browser = new FolderBrowser(String)
      await browser.open()

      expect(browser.typed).toBe('/home/u')
      await expect(browser.chosenPath()).resolves.toBe('/home/u')
      expect(asked).toEqual(['/v1/fs'])
    })

    it('lists a path typed over it, trimmed, before giving it', async () => {
      const asked = serve(
        () => json(listing('/home/u', [])),
        () => json(listing('/srv/data', [], '/srv')),
      )
      const browser = new FolderBrowser(String)
      await browser.open()

      browser.typed = '  /srv/data \n'
      await expect(browser.chosenPath()).resolves.toBe('/srv/data')

      expect(asked).toEqual(['/v1/fs', '/v1/fs?path=/srv/data'])
      expect(browser.path).toBe('/srv/data')
    })

    it('gives nothing for a path typed over it that cannot be listed', async () => {
      serve(
        () => json(listing('/home/u', [])),
        () => problem(404, 'path_not_found', 'nothing there'),
      )
      const browser = new FolderBrowser(String)
      await browser.open()

      browser.typed = '/gone'
      await expect(browser.chosenPath()).resolves.toBeNull()

      expect(browser.error).not.toBe('')
      expect(browser.path).toBe('/home/u')
    })

    it('gives nothing when a newer listing overtook the one of the path typed', async () => {
      const answers: ((body: DirectoryListing) => void)[] = []
      vi.stubGlobal('fetch', () => new Promise<Response>((resolve) => answers.push((body) => resolve(json(body)))))
      const browser = new FolderBrowser(String)

      browser.typed = '/srv/data'
      const chosen = browser.chosenPath()
      const newer = browser.open('/home/u/b')
      answers[1]?.(listing('/home/u/b', []))
      answers[0]?.(listing('/srv/data', []))
      await newer

      await expect(chosen).resolves.toBeNull()
    })

    it('cannot be added from while the newest listing was refused, or the field is empty', async () => {
      serve(
        () => json(listing('/home/u', [])),
        () => problem(404, 'path_not_found', 'nothing there'),
      )
      const browser = new FolderBrowser(String)
      expect(browser.canChoose).toBe(false)
      await browser.open()
      expect(browser.canChoose).toBe(true)

      browser.typed = ' '
      expect(browser.canChoose).toBe(false)
      browser.typed = '/gone'
      await browser.open('/gone')

      expect(browser.canChoose).toBe(false)
    })

    it('starts over from the directory listed, forgetting a refusal and what was typed', async () => {
      const asked = serve(
        () => json(listing('/home/u', [folder('/home/u/src')])),
        () => problem(404, 'path_not_found', 'nothing there'),
      )
      const browser = new FolderBrowser(String)
      await browser.open()
      browser.typed = '/gone'
      await browser.open('/gone')

      browser.startOver()

      expect(browser.error).toBe('')
      expect(browser.typed).toBe('/home/u')
      expect(browser.directories.map((d) => d.name)).toEqual(['src'])
      expect(browser.canChoose).toBe(true)
      expect(asked).toHaveLength(2)
    })

    it('gives nothing for an empty field', async () => {
      const asked = serve(() => json(listing('/home/u', [])))
      const browser = new FolderBrowser(String)
      await browser.open()

      browser.typed = '   '

      await expect(browser.chosenPath()).resolves.toBeNull()
      expect(asked).toEqual(['/v1/fs'])
    })
  })

  it('has nowhere to go up to at the top', async () => {
    const asked = serve(() => json(listing('/', [folder('/home')], null)))
    const browser = new FolderBrowser(String)
    await browser.open()

    expect(browser.canGoUp).toBe(false)
    await browser.up()

    expect(asked).toEqual(['/v1/fs'])
  })
})

describe('where the folder browser starts', () => {
  it('lists the place it was told to start at', async () => {
    const asked = serve(() => json(listing('/home/u', [folder('/home/u/src')])))
    const browser = new FolderBrowser(String, '/home/u')

    await browser.begin()

    expect(asked).toEqual(['/v1/fs?path=/home/u'])
    expect(browser.path).toBe('/home/u')
    expect(browser.error).toBe('')
  })

  it('lists the home directory instead, saying nothing of it, when a top it starts at cannot be listed', async () => {
    const asked = serve(
      () => problem(403, 'path_unreadable', '/ cannot be listed'),
      () => json(listing('/home/u', [])),
    )
    const failure = vi.fn(String)
    const browser = new FolderBrowser(failure, '/')

    await browser.begin()

    expect(asked).toEqual(['/v1/fs?path=/', '/v1/fs'])
    expect(browser.path).toBe('/home/u')
    expect(failure).not.toHaveBeenCalled()
    expect(browser.error).toBe('')
  })

  it('climbs to the nearest place above it that can be listed, saying nothing of those that cannot', async () => {
    const asked = serve(
      () => problem(404, 'path_not_found', '/home/u/gone/src does not exist'),
      () => problem(404, 'path_not_found', '/home/u/gone does not exist'),
      () => json(listing('/home/u', [folder('/home/u/src')])),
    )
    const failure = vi.fn(String)
    const browser = new FolderBrowser(failure, '/home/u/gone/src')

    await browser.begin()

    expect(asked).toEqual(['/v1/fs?path=/home/u/gone/src', '/v1/fs?path=/home/u/gone', '/v1/fs?path=/home/u'])
    expect(browser.path).toBe('/home/u')
    expect(failure).not.toHaveBeenCalled()
    expect(browser.error).toBe('')
  })

  it('lists the home directory once nothing up to the top can be listed', async () => {
    const asked = serve(
      () => problem(404, 'path_not_found', '/mnt/gone/src does not exist'),
      () => problem(404, 'path_not_found', '/mnt/gone does not exist'),
      () => problem(403, 'path_unreadable', '/mnt cannot be listed'),
      () => problem(403, 'path_unreadable', '/ cannot be listed'),
      () => json(listing('/home/u', [])),
    )
    const failure = vi.fn(String)
    const browser = new FolderBrowser(failure, '/mnt/gone/src')

    await browser.begin()

    expect(asked).toEqual([
      '/v1/fs?path=/mnt/gone/src',
      '/v1/fs?path=/mnt/gone',
      '/v1/fs?path=/mnt',
      '/v1/fs?path=/',
      '/v1/fs',
    ])
    expect(browser.path).toBe('/home/u')
    expect(failure).not.toHaveBeenCalled()
    expect(browser.error).toBe('')
  })

  it('stops climbing once a newer listing is asked for, keeping that one', async () => {
    const asked: string[] = []
    const answers: (() => void)[] = []
    vi.stubGlobal('fetch', (url: string) => {
      asked.push(url)
      return new Promise<Response>((resolve) =>
        answers.push(() =>
          resolve(url.endsWith('gone') ? problem(404, 'path_not_found', 'gone') : json(listing('/home/u/b', []))),
        ),
      )
    })
    const browser = new FolderBrowser(String, '/home/u/gone')

    const starting = browser.begin()
    const opening = browser.open('/home/u/b')
    answers[1]?.()
    answers[0]?.()
    await Promise.all([starting, opening])

    expect(asked).toEqual(['/v1/fs?path=/home/u/gone', '/v1/fs?path=/home/u/b'])
    expect(browser.path).toBe('/home/u/b')
  })

  it('lists the home directory when told nowhere to start', async () => {
    const asked = serve(() => json(listing('/home/u', [])))
    const browser = new FolderBrowser(String)

    await browser.begin()

    expect(asked).toEqual(['/v1/fs'])
  })
})

describe('the directory above a path on the daemon’s machine', () => {
  it.each([
    ['/home/u/src', '/home/u'],
    ['/home/u/src/', '/home/u'],
    ['/src', '/'],
    ['C:\\Users\\u\\src', 'C:\\Users\\u'],
    ['C:\\src', 'C:\\'],
    ['\\\\server\\share\\dir', '\\\\server\\share\\'],
    ['\\\\server\\share\\dir\\', '\\\\server\\share\\'],
    ['\\\\server\\share\\a\\b', '\\\\server\\share\\a'],
  ])('is the parent of %s: %s', (path, parent) => {
    expect(parentDirectory(path)).toBe(parent)
  })

  it.each(['/', 'C:\\', 'src', '\\\\server\\share', '\\\\server\\share\\', '\\\\server'])('is none for %s', (path) => {
    expect(parentDirectory(path)).toBeNull()
  })
})
