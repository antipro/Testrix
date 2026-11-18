import { getProjectAccessToken, isAuthorized, jsonResponse } from '../../../_shared.js';

export async function onRequestPost({ request, env, params }) {
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
  const body = await request.json().catch(() => ({}));
  const provided = String(body.accessToken || '');
  if (requiredToken && provided !== requiredToken) {
    return jsonResponse({ error: 'Invalid access token' }, 401);
  }

  return jsonResponse(
    { ok: true },
    200,
    {
      'Set-Cookie': `testrix_cf_access_${slug}=${encodeURIComponent(provided)}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800`,
    }
  );
}
