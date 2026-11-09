// The release workflow publishes these versionless names on every release, and
// GitHub redirects latest/download to the newest one.
const latest = 'https://github.com/raine/aven/releases/latest/download';

export const macAppleSilicon = `${latest}/Aven-Desktop-arm64.dmg`;
export const macIntel = `${latest}/Aven-Desktop-amd64.dmg`;
