/**
 * Model profiles as the runtime sees them (spec section 10.6): the bundle,
 * with `JEVSCRIPT_PROFILES` layered over it, a profile with the same id
 * replacing the bundled one. The toolbox only displays them; the runtime
 * resolves them itself at `task.start`.
 */
import { readFile } from 'node:fs/promises'

import type { ProfileInfo } from '../shared/protocol.ts'
import type { Profile } from '../shared/recording.ts'

export async function readProfiles(
  bundledPath: string,
  overlayPath: string | undefined,
): Promise<{ profiles: ProfileInfo[]; overlay: string | null }> {
  const byModel = new Map<string, ProfileInfo>()
  for (const profile of JSON.parse(await readFile(bundledPath, 'utf8')) as Profile[]) {
    byModel.set(profile.model, { ...profile, source: 'bundled' })
  }
  if (overlayPath) {
    for (const profile of JSON.parse(await readFile(overlayPath, 'utf8')) as Profile[]) {
      byModel.set(profile.model, { ...profile, source: 'overlay' })
    }
  }
  return { profiles: [...byModel.values()], overlay: overlayPath ?? null }
}

/** Follow one level of alias, as `Profiles::resolve` does. */
export function resolveProfile(profiles: readonly ProfileInfo[], model: string): ProfileInfo | undefined {
  const profile = profiles.find((candidate) => candidate.model === model)
  if (!profile?.aliases) return profile
  return profiles.find((candidate) => candidate.model === profile.aliases)
}
