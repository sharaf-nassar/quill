(function () {
	const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

	function setupGsap() {
		if (reduceMotion || !window.gsap || !window.ScrollTrigger) return;

		const gsap = window.gsap;
		const ScrollTrigger = window.ScrollTrigger;
		gsap.registerPlugin(ScrollTrigger);

		gsap.utils.toArray(".motion-rise").forEach((item) => {
			gsap.fromTo(item, {
				y: 48,
				scale: 0.985
			}, {
				y: 0,
				scale: 1,
				duration: 0.9,
				ease: "power3.out",
				scrollTrigger: {
					trigger: item,
					start: "top 92%",
					end: "top 56%",
					scrub: 0.7
				}
			});
		});
	}

	setupGsap();
})();
