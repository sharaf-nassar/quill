(function () {
	const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
	if (reduceMotion || !("IntersectionObserver" in window)) return;

	const observer = new IntersectionObserver((entries) => {
		entries.forEach((entry) => {
			if (!entry.isIntersecting) return;
			entry.target.classList.add("is-visible");
			observer.unobserve(entry.target);
		});
	}, { rootMargin: "0px 0px -8%" });

	document.querySelectorAll(".motion-rise").forEach((item) => {
		item.classList.add("motion-pending");
		observer.observe(item);
	});
})();
