import { jsonResponse } from '../_shared.js';

export async function onRequestPost({ request, env }) {
  const expected = env.ACCESS_PASSCODE;
  if (!expected) {
    return jsonResponse({ ok: true, required: false });
  }
  const body = await request.json().catch(() => ({}));
  if (String(body.passcode || '') !== expected) {
    return jsonResponse({ error: 'Invalid passcode' }, 401);
  }
  return jsonResponse(
    { ok: true },
    200,
    {
      'Set-Cookie': `testrix_cf_pass=${encodeURIComponent(expected)}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800`,
    }
  );
}
