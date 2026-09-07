const status = document.createElement("div");
status.className = "docs-copy-status";
status.setAttribute("role", "status");
status.setAttribute("aria-live", "polite");
status.setAttribute("aria-atomic", "true");
document.body.append(status);

let hideStatus;

function copyWithTextArea(url) {
  const textArea = document.createElement("textarea");
  textArea.value = url;
  textArea.setAttribute("readonly", "");
  textArea.style.position = "fixed";
  textArea.style.opacity = "0";
  document.body.append(textArea);
  textArea.select();

  try {
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    textArea.remove();
  }
}

async function copyLink(url) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(url);
      return true;
    } catch {
      return copyWithTextArea(url);
    }
  }

  return copyWithTextArea(url);
}

function showStatus(message) {
  window.clearTimeout(hideStatus);
  status.textContent = message;
  status.classList.add("is-visible");
  hideStatus = window.setTimeout(() => {
    status.classList.remove("is-visible");
  }, 1800);
}

for (const anchor of document.querySelectorAll(".heading-anchor")) {
  anchor.addEventListener("click", async () => {
    const url = new URL(anchor.href, document.baseURI).href;
    showStatus(
      (await copyLink(url))
        ? "Link copied"
        : "Link wasn't copied. Copy the URL from your address bar.",
    );
  });
}
