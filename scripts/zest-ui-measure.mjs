// Runs in the fixture browser, shared by the control CLI and regression tests.
export function measureUiElements(selector = 'button,[role="dialog"],input,textarea,p,label') {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) throw new Error("UI measurements require a canvas context");
  const rgba = (color) => {
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = color;
    context.fillRect(0, 0, 1, 1);
    const [r, g, b, alpha] = context.getImageData(0, 0, 1, 1).data;
    return [r, g, b, alpha / 255];
  };
  const over = (front, back) => front.slice(0, 3).map((channel, index) => channel * front[3] + back[index] * (1 - front[3]));
  const luminance = (rgb) => {
    const linear = rgb.map((channel) => {
      const value = channel / 255;
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
    return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
  };
  const contrast = (element) => {
    for (let ancestor = element; ancestor; ancestor = ancestor.parentElement) {
      const style = getComputedStyle(ancestor);
      if (Number(style.opacity) !== 1 || style.filter !== "none") {
        return { contrast: null, contrastUnknown: "filter or opacity requires visual review" };
      }
    }
    const layers = [];
    for (let ancestor = element; ancestor; ancestor = ancestor.parentElement) {
      const style = getComputedStyle(ancestor);
      if (style.backgroundImage !== "none" || style.backdropFilter !== "none") {
        return { contrast: null, contrastUnknown: "image, filter, or opacity requires visual review" };
      }
      const color = rgba(style.backgroundColor);
      layers.push(color);
      if (color[3] === 1) {
        let background = color.slice(0, 3);
        for (let index = layers.length - 2; index >= 0; index -= 1) background = over(layers[index], background);
        const foreground = over(rgba(getComputedStyle(element).color), background);
        const first = luminance(foreground);
        const second = luminance(background);
        return { contrast: (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05), contrastUnknown: null };
      }
    }
    return { contrast: null, contrastUnknown: "no opaque background" };
  };
  return [...document.querySelectorAll(selector)].filter((element) => element.checkVisibility({ opacityProperty: true, visibilityProperty: true })).map((element) => {
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    const clipping = [];
    for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
      const bounds = ancestor.getBoundingClientRect();
      const parentStyle = getComputedStyle(ancestor);
      if ((parentStyle.overflowX !== "visible" && (rect.left < bounds.left - 1 || rect.right > bounds.right + 1)) ||
          (parentStyle.overflowY !== "visible" && (rect.top < bounds.top - 1 || rect.bottom > bounds.bottom + 1))) {
        clipping.push(ancestor.getAttribute("aria-label") || ancestor.tagName.toLowerCase());
      }
    }
    return {
      name: element.getAttribute("aria-label") || element.getAttribute("title") || element.textContent?.trim().slice(0, 80) || element.tagName.toLowerCase(),
      bounds: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
      inViewport: rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight,
      clipping,
      fontSize: Number.parseFloat(style.fontSize),
      color: style.color,
      ...contrast(element),
    };
  });
}
