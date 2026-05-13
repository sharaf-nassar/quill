(function () {
	const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

	function setupCarousel() {
		const root = document.querySelector("[data-carousel]");
		if (!root) return;

		const slides = Array.from(root.querySelectorAll(".note-slide"));
		const prev = root.querySelector("[data-carousel-prev]");
		const next = root.querySelector("[data-carousel-next]");
		let activeIndex = Math.max(0, slides.findIndex((slide) => slide.classList.contains("is-active")));

		function show(index) {
			activeIndex = (index + slides.length) % slides.length;
			slides.forEach((slide, slideIndex) => {
				const active = slideIndex === activeIndex;
				slide.classList.toggle("is-active", active);
				slide.setAttribute("aria-hidden", active ? "false" : "true");
			});
		}

		prev?.addEventListener("click", () => show(activeIndex - 1));
		next?.addEventListener("click", () => show(activeIndex + 1));
		show(activeIndex);
	}

	function splitScrubText() {
		document.querySelectorAll("[data-scrub-text]").forEach((node) => {
			const text = node.textContent || "";
			const words = text.trim().split(/\s+/);
			node.textContent = "";

			words.forEach((word, index) => {
				const span = document.createElement("span");
				span.className = "scrub-word";
				span.textContent = word;
				node.appendChild(span);
				if (index < words.length - 1) {
					node.appendChild(document.createTextNode(" "));
				}
			});
		});
	}

	function setupGsap() {
		if (reduceMotion || !window.gsap || !window.ScrollTrigger) return;

		const gsap = window.gsap;
		const ScrollTrigger = window.ScrollTrigger;
		gsap.registerPlugin(ScrollTrigger);
		splitScrubText();

		gsap.utils.toArray(".motion-rise").forEach((item) => {
			gsap.fromTo(item, {
				y: 52,
				scale: 0.97
			}, {
				y: 0,
				scale: 1,
				duration: 1,
				ease: "power3.out",
				scrollTrigger: {
					trigger: item,
					start: "top 92%",
					end: "top 50%",
					scrub: 0.8
				}
			});
		});

		gsap.utils.toArray(".scrub-word").forEach((word, index, words) => {
			gsap.to(word, {
				opacity: 1,
				ease: "none",
				scrollTrigger: {
					trigger: ".proof-run",
					start: `top+=${index * 10} 72%`,
					end: `top+=${(index + words.length) * 10} 34%`,
					scrub: true
				}
			});
		});

		ScrollTrigger.matchMedia({
			"(min-width: 981px)": function () {
				ScrollTrigger.create({
					trigger: ".proof-run",
					pin: ".proof-copy",
					start: "top 92px",
					end: "bottom bottom",
					pinSpacing: false,
					anticipatePin: 1
				});
			}
		});
	}

	setupCarousel();
	setupGsap();
})();
