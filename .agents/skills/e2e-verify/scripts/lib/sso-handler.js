async function detectSSOPage() {
  return false;
}

async function waitForSSOLogin() {
  return false;
}

async function ensureLoggedIn() {
  return true;
}

async function handleSSOIfNeeded() {
  return false;
}

module.exports = {
  handleSSOIfNeeded,
  detectSSOPage,
  waitForSSOLogin,
  ensureLoggedIn,
};
