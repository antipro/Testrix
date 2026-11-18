import {
  buildProjectTree,
  getProjectAccessToken,
  isAuthorized,
  jsonResponse,
  listAllObjects,
  parseCookies,
  parseYamlSafe,
} from '../../_shared.js';

export async function onRequestGet({ request, env, params }) {
  if (!isAuthorized(request, env)) {
    return jsonResponse({ error: 'Passcode required', authRequired: true }, 401);
  }
  if (!env.TESTRIX_BUCKET) {
    return jsonResponse({ error: 'TESTRIX_BUCKET R2 binding is not configured' }, 500);
  }

  const slug = decodeURIComponent(String(params.slug || '')).replace(/[^a-zA-Z0-9-_]/g, '');
  if (!slug) {
    return jsonResponse({ error: 'Invalid project slug' }, 400);
  }

  const requiredToken = await getProjectAccessToken(env.TESTRIX_BUCKET, slug);
  if (requiredToken) {
    const url = new URL(request.url);
    const cookies = parseCookies(request.headers.get('Cookie'));
    const provided =
      request.headers.get('X-Access-Token') ||
      url.searchParams.get('token') ||
      cookies[`testrix_cf_access_${slug}`] ||
      '';
    if (String(provided) !== requiredToken) {
      return jsonResponse(
        { error: 'Access token required', accessTokenRequired: true },
        403
      );
    }
  }

  const prefix = `${slug}/`;
  const objects = await listAllObjects(env.TESTRIX_BUCKET, prefix);
  const yamlObjects = objects.filter((o) => o.key.endsWith('.yaml') || o.key.endsWith('.yml'));

  const fileMap = new Map();
  await Promise.all(
    yamlObjects.map(async (obj) => {
      const relPath = obj.key.slice(prefix.length);
      const body = await env.TESTRIX_BUCKET.get(obj.key);
      if (body) {
        const doc = parseYamlSafe(await body.text());
        if (doc) fileMap.set(relPath, doc);
      }
    })
  );

  const project = buildProjectTree(slug, fileMap);
  if (!project) {
    return jsonResponse({ error: 'Project not found in R2 bucket' }, 404);
  }

  return jsonResponse({ project });
}
