document.documentElement.classList.add("has-js");

document.querySelectorAll("[data-copy-target]").forEach((button) => {
  const idleLabel = button.getAttribute("aria-label");
  const commandName = button.dataset.copyLabel;
  let reset;

  button.addEventListener("click", async () => {
    const command = document.getElementById(button.dataset.copyTarget).textContent;

    try {
      await navigator.clipboard.writeText(command);
      button.textContent = "Copied";
      button.setAttribute("aria-label", `${commandName} command copied`);
    } catch {
      button.textContent = "Select command";
      button.setAttribute("aria-label", `Copy failed. Select the ${commandName.toLowerCase()} command`);
    }

    clearTimeout(reset);
    reset = setTimeout(() => {
      button.textContent = "Copy";
      button.setAttribute("aria-label", idleLabel);
    }, 1800);
  });
});
