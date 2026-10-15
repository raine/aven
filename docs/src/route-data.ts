import { defineRouteMiddleware } from '@astrojs/starlight/route-data';
import type { StarlightRouteData } from '@astrojs/starlight/route-data';

type SidebarEntry = StarlightRouteData['sidebar'][number];

// The sidebar links only the CLI changelog; the platform changelogs under
// /changelog/ highlight that link too.
export const onRequest = defineRouteMiddleware((context) => {
  const route = context.locals.starlightRoute;
  if (!route.id.startsWith('changelog/')) return;
  markCurrent(route.sidebar, '/changelog/');
});

function markCurrent(entries: SidebarEntry[], href: string) {
  for (const entry of entries) {
    if (entry.type === 'group') markCurrent(entry.entries, href);
    else if (entry.href === href) entry.isCurrent = true;
  }
}
