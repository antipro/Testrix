import { isAuthorized, jsonResponse, listAllObjects, parseYamlSafe } from '../_shared.js';

export async function onRequestGet({ request, env }) {
  if (!isAuthorized(request, env)) {
    return jsonResponse({ error: 'Passcode required', authRequired: true }, 401);
  }
  if (!env.TESTRIX_BUCKET) {
    return jsonResponse({ error: 'TESTRIX_BUCKET R2 binding is not configured' }, 500);
  }

  const objects = await listAllObjects(env.TESTRIX_BUCKET);
  const manifestObjs = objects.filter((o) => o.key.endsWith('/testrix.project.yaml'));

  const projects = [];
  for (const obj of manifestObjs) {
    const slug = obj.key.slice(0, -'/testrix.project.yaml'.length);
    if (!slug || slug.includes('/')) continue;
    const item = await env.TESTRIX_BUCKET.get(obj.key);
    if (!item) continue;
    const doc = parseYamlSafe(await item.text());
    if (doc) {
      projects.push({
        slug,
        name: doc.name || slug,
        description: doc.description || '',
      });
    }
  }

  projects.sort((a, b) => a.name.localeCompare(b.name));
  return jsonResponse({ projects });
}
