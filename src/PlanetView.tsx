import { useEffect, useMemo, useRef, useState } from 'react';
import type { PlanetSnapshot } from './game';

// A procedural illustration, not simulation data and not a geographic Earth map.
function createSurface(
  seed: number,
  oceanRgb: [number, number, number],
  landRgb: [number, number, number],
): HTMLCanvasElement | null {
  if (typeof document === 'undefined') return null;
  const texture = document.createElement('canvas');
  texture.width = texture.height = 512;
  const context = texture.getContext('2d');
  if (!context) return null;
  const image = context.createImageData(512, 512);
  const phase = seed * 0.137;
  for (let y = 0; y < 512; y++) {
    for (let x = 0; x < 512; x++) {
      const nx = (x - 255.5) / 255;
      const ny = (y - 255.5) / 255;
      const squared = nx * nx + ny * ny;
      if (squared >= 1) continue;
      const nz = Math.sqrt(1 - squared);
      const longitude = Math.atan2(nx, nz);
      const latitude = Math.asin(ny);
      const terrain = Math.sin(longitude * 5 + phase + Math.sin(latitude * 6))
        + Math.cos(latitude * 8 - longitude * 3)
        + 0.4 * Math.sin(longitude * 19 + latitude * 13);
      const land = terrain > 0.65;
      const color = land ? landRgb : oceanRgb;
      const light = Math.max(0.055, -nx * 0.55 - ny * 0.35 + nz * 0.7);
      const cloud = Math.max(0, Math.sin(latitude * 26 + Math.sin(longitude * 9 + phase) * 3)
        + Math.sin(longitude * 14 - latitude * 8) - 1.4) * 0.65;
      const ice = Math.max(0, (Math.abs(latitude) - 1.05) * 2);
      const white = Math.min(0.85, cloud + ice);
      const rim = Math.pow(1 - nz, 4);
      const index = (y * 512 + x) * 4;
      for (let channel = 0; channel < 3; channel++) {
        const base = color[channel]! * (1 - white) + 220 * white;
        image.data[index + channel] = Math.min(255, base * light + rim * [20, 42, 60][channel]!);
      }
      image.data[index + 3] = Math.min(255, (1 - squared) * 255 * 180);
    }
  }
  context.putImageData(image, 0, 0);
  return texture;
}

export interface PlanetViewProps {
  planet: PlanetSnapshot;
  onSelectPlanet?: () => void;
  isWindowOpen?: boolean;
}

export function PlanetView({ planet, onSelectPlanet, isWindowOpen = false }: PlanetViewProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const [globeBox, setGlobeBox] = useState<{ cx: number; cy: number; radius: number }>({
    cx: 640,
    cy: 400,
    radius: 160,
  });

  const { seed, ocean_rgb: ocean, land_rgb: land } = planet.appearance;
  const oceanKey = `${ocean[0]},${ocean[1]},${ocean[2]}`;
  const landKey = `${land[0]},${land[1]},${land[2]}`;

  // Memoized on primitive appearance fields so ticking clock snapshots never regenerate the 512x512 texture.
  const surface = useMemo(
    () => createSurface(seed, ocean, land),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [seed, oceanKey, landKey],
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const context = canvas.getContext('2d');
    if (!context || !surface) {
      if (errorRef.current) errorRef.current.hidden = false;
      return;
    }

    const draw = () => {
      const { width, height } = canvas.getBoundingClientRect();
      if (width === 0 || height === 0) return;
      const scale = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(width * scale);
      canvas.height = Math.round(height * scale);
      context.setTransform(scale, 0, 0, scale, 0, 0);
      context.fillStyle = '#090e18';
      context.fillRect(0, 0, width, height);

      // Fixed decorative stars: resize does not change their arrangement.
      let starSeed = seed;
      const random = () => {
        starSeed = (Math.imul(starSeed, 1664525) + 1013904223) >>> 0;
        return starSeed / 4294967296;
      };
      for (let index = 0; index < 240; index++) {
        const x = random() * width;
        const y = random() * height;
        const alpha = 0.12 + random() * 0.48;
        context.fillStyle = `rgba(198, 214, 229, ${alpha})`;
        context.beginPath();
        context.arc(x, y, random() > 0.95 ? 1.2 : 0.65, 0, Math.PI * 2);
        context.fill();
      }

      const cx = width * 0.5;
      const cy = height * 0.5;
      const radius = Math.max(88, Math.min(width * 0.22, height * 0.27));
      setGlobeBox((prev) =>
        Math.abs(prev.cx - cx) < 1 && Math.abs(prev.cy - cy) < 1 && Math.abs(prev.radius - radius) < 1
          ? prev
          : { cx, cy, radius },
      );

      const halo = context.createRadialGradient(cx, cy, radius * 0.94, cx, cy, radius * 1.24);
      halo.addColorStop(0, 'rgba(108, 169, 199, 0.22)');
      halo.addColorStop(0.5, 'rgba(70, 122, 161, 0.055)');
      halo.addColorStop(1, 'rgba(70, 122, 161, 0)');
      context.fillStyle = halo;
      context.beginPath();
      context.arc(cx, cy, radius * 1.24, 0, Math.PI * 2);
      context.fill();
      context.drawImage(surface, cx - radius, cy - radius, radius * 2, radius * 2);
    };

    const observer = new ResizeObserver(draw);
    observer.observe(canvas);
    window.addEventListener('resize', draw);
    draw();
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', draw);
    };
  }, [surface, seed]);

  return (
    <>
      <canvas
        ref={canvasRef}
        className="planet-canvas"
        role="img"
        aria-label={`${planet.name}的二维示意图，包含海洋、陆地和大气光晕。`}
      />
      {onSelectPlanet && (
        <button
          type="button"
          className={isWindowOpen ? 'planet-globe-hit window-open' : 'planet-globe-hit'}
          data-testid="earth-globe-trigger"
          aria-label={`点击查看${planet.name}地表窗口`}
          style={{
            left: `${Math.round(globeBox.cx - globeBox.radius)}px`,
            top: `${Math.round(globeBox.cy - globeBox.radius)}px`,
            width: `${Math.round(globeBox.radius * 2)}px`,
            height: `${Math.round(globeBox.radius * 2)}px`,
          }}
          onClick={onSelectPlanet}
        >
          {!isWindowOpen && (
            <span className="planet-globe-badge">
              <strong>{planet.name}</strong>
              <small>
                {planet.polity.name} · 人口 {planet.total_population.toLocaleString('zh-CN')} · 点击展开地表
              </small>
            </span>
          )}
        </button>
      )}
      <p ref={errorRef} hidden className="render-error" role="alert">
        当前环境不支持 Canvas 2D，无法绘制星球。
      </p>
    </>
  );
}
