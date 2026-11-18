const nodemailer = require('nodemailer');

let cachedTransporter = null;

function getTransporter() {
  if (cachedTransporter) return cachedTransporter;
  const host = (process.env.SMTP_HOST || '').trim();
  if (!host) return null;

  const port = Number(process.env.SMTP_PORT || 587);
  const secure = String(process.env.SMTP_SECURE || '').toLowerCase() === 'true' || port === 465;
  const user = (process.env.SMTP_USER || '').trim();
  const pass = process.env.SMTP_PASS || '';

  cachedTransporter = nodemailer.createTransport({
    host,
    port,
    secure,
    auth: user ? { user, pass } : undefined,
  });
  return cachedTransporter;
}

async function sendVerificationCode(toEmail, code) {
  const transporter = getTransporter();
  if (!transporter) {
    if (process.env.SMTP_DEV_LOG === '1') {
      console.log(`[DEV SMTP] Verification code for ${toEmail}: ${code}`);
      return;
    }
    throw new Error('SMTP is not configured (set SMTP_HOST, SMTP_PORT, SMTP_USER, SMTP_PASS in .env)');
  }

  const from = process.env.SMTP_FROM || process.env.SMTP_USER || 'noreply@testrix.local';
  await transporter.sendMail({
    from,
    to: toEmail,
    subject: 'Testrix Share - Email Verification Code',
    text: `Your Testrix Share verification code is: ${code}\n\nThis code expires in 10 minutes.`,
    html: `<p>Your Testrix Share verification code is: <strong style="font-size:18px;letter-spacing:2px;">${code}</strong></p><p>This code expires in 10 minutes.</p>`,
  });
}

module.exports = { sendVerificationCode };
