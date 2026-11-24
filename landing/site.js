document.documentElement.classList.add("has-js");

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

// The header gets its frosted bar once the page moves under it.
const header = document.querySelector(".site-header");
if (header) {
  const markScrolled = () => header.classList.toggle("is-scrolled", window.scrollY > 8);
  markScrolled();
  window.addEventListener("scroll", markScrolled, { passive: true });
}

// Sections fade up once as they enter the screen.
const revealed = document.querySelectorAll(".reveal");
if ("IntersectionObserver" in window) {
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const element = entry.target;
        element.classList.add("is-in");
        observer.unobserve(element);
        // Drop the stagger so later hover transitions start at once.
        setTimeout(() => element.style.removeProperty("transition-delay"), 600);
      }
    },
    { rootMargin: "0px 0px -8% 0px" },
  );
  // Siblings that enter together start 60ms apart.
  revealed.forEach((element) => {
    const index = [...element.parentElement.children].filter((child) => child.classList.contains("reveal")).indexOf(element);
    if (index > 0 && !reduceMotion.matches) element.style.transitionDelay = `${Math.min(index, 4) * 60}ms`;
    observer.observe(element);
  });
} else {
  revealed.forEach((element) => element.classList.add("is-in"));
}

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
