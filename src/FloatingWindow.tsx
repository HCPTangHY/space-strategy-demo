import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';

type ResizeDir = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';

export interface WindowRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FloatingWindowProps {
  title: ReactNode;
  headerActions?: ReactNode;
  children: ReactNode;
  onClose?: () => void;
  onFocus?: () => void;
  zIndex?: number;
  minWidth?: number;
  minHeight?: number;
  topSafeInset?: number;
  bottomSafeInset?: number;
  defaultRect?: (viewportW: number, viewportH: number) => WindowRect;
  testId?: string;
}

const RESIZE_DIRS: readonly ResizeDir[] = ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'];

function clampRect(
  rect: WindowRect,
  vw: number,
  vh: number,
  minW: number,
  minH: number,
  topSafe: number,
  bottomSafe: number,
): WindowRect {
  const maxW = Math.max(320, vw - 16);
  const maxH = Math.max(260, vh - topSafe - bottomSafe);
  const width = Math.min(maxW, Math.max(Math.min(minW, maxW), Math.round(rect.width)));
  const height = Math.min(maxH, Math.max(Math.min(minH, maxH), Math.round(rect.height)));
  const x = Math.min(Math.max(8, Math.round(rect.x)), Math.max(8, vw - width - 8));
  const y = Math.min(Math.max(topSafe, Math.round(rect.y)), Math.max(topSafe, vh - bottomSafe - height));
  return { x, y, width, height };
}

function defaultComputeRect(
  vw: number,
  vh: number,
  topSafe: number,
  bottomSafe: number,
): WindowRect {
  const width = Math.round(Math.min(1320, vw * 0.88));
  const height = Math.round(Math.min(860, (vh - topSafe - bottomSafe) * 0.92));
  const x = Math.round((vw - width) * 0.5);
  const y = Math.round(topSafe + (vh - topSafe - bottomSafe - height) * 0.45);
  return { x, y, width, height };
}

export function FloatingWindow({
  title,
  headerActions,
  children,
  onClose,
  onFocus,
  zIndex = 10,
  minWidth = 620,
  minHeight = 400,
  topSafeInset = 76,
  bottomSafeInset = 44,
  defaultRect,
  testId = 'floating-window',
}: FloatingWindowProps) {
  const [viewport, setViewport] = useState<{ w: number; h: number }>(() => ({
    w: typeof window !== 'undefined' ? window.innerWidth : 1280,
    h: typeof window !== 'undefined' ? window.innerHeight : 800,
  }));
  const isCompact = viewport.w < 760;

  const [rect, setRect] = useState<WindowRect>(() => {
    const vw = typeof window !== 'undefined' ? window.innerWidth : 1280;
    const vh = typeof window !== 'undefined' ? window.innerHeight : 800;
    const initial = defaultRect
      ? defaultRect(vw, vh)
      : defaultComputeRect(vw, vh, topSafeInset, bottomSafeInset);
    return clampRect(initial, vw, vh, minWidth, minHeight, topSafeInset, bottomSafeInset);
  });

  const [maximized, setMaximized] = useState(false);
  const preMaxRectRef = useRef<WindowRect>(rect);

  const actionRef = useRef<
    | { kind: 'move'; startX: number; startY: number; startRect: WindowRect }
    | { kind: 'resize'; dir: ResizeDir; startX: number; startY: number; startRect: WindowRect }
    | null
  >(null);

  useEffect(() => {
    const onResize = () => {
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      setViewport({ w: vw, h: vh });
      setRect((prev) =>
        clampRect(prev, vw, vh, minWidth, minHeight, topSafeInset, bottomSafeInset),
      );
    };
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [bottomSafeInset, minHeight, minWidth, topSafeInset]);

  const toggleMaximize = useCallback(() => {
    setMaximized((prev) => {
      if (!prev) {
        preMaxRectRef.current = rect;
      }
      return !prev;
    });
  }, [rect]);

  const startMove = (ev: React.PointerEvent<HTMLDivElement>) => {
    if (isCompact || maximized || ev.button !== 0) return;
    const target = ev.target as HTMLElement | null;
    if (
      target?.closest('button, select, input, textarea, a, [data-no-window-drag="true"]')
    ) {
      return;
    }
    onFocus?.();
    actionRef.current = {
      kind: 'move',
      startX: ev.clientX,
      startY: ev.clientY,
      startRect: rect,
    };
    try {
      ev.currentTarget.setPointerCapture(ev.pointerId);
    } catch {
      // Ignore synthetic pointer capture error.
    }
  };

  const startResize = (dir: ResizeDir) => (ev: React.PointerEvent<HTMLDivElement>) => {
    if (isCompact || ev.button !== 0) return;
    ev.stopPropagation();
    onFocus?.();
    const baseRect = maximized
      ? {
          x: 12,
          y: topSafeInset,
          width: viewport.w - 24,
          height: viewport.h - topSafeInset - bottomSafeInset,
        }
      : rect;
    if (maximized) {
      setMaximized(false);
      setRect(baseRect);
    }
    actionRef.current = {
      kind: 'resize',
      dir,
      startX: ev.clientX,
      startY: ev.clientY,
      startRect: baseRect,
    };
    try {
      ev.currentTarget.setPointerCapture(ev.pointerId);
    } catch {
      // Ignore synthetic pointer capture error.
    }
  };

  const onPointerMove = (ev: React.PointerEvent<HTMLDivElement>) => {
    const act = actionRef.current;
    if (!act) return;
    const dx = ev.clientX - act.startX;
    const dy = ev.clientY - act.startY;
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    if (act.kind === 'move') {
      setRect(
        clampRect(
          {
            ...act.startRect,
            x: act.startRect.x + dx,
            y: act.startRect.y + dy,
          },
          vw,
          vh,
          minWidth,
          minHeight,
          topSafeInset,
          bottomSafeInset,
        ),
      );
      return;
    }

    const s = act.startRect;
    const effMinW = Math.min(minWidth, vw - 16);
    const effMinH = Math.min(minHeight, vh - topSafeInset - bottomSafeInset);
    let left = s.x;
    let right = s.x + s.width;
    let top = s.y;
    let bottom = s.y + s.height;

    if (act.dir.includes('e')) {
      right = Math.min(vw - 8, Math.max(left + effMinW, right + dx));
    }
    if (act.dir.includes('w')) {
      left = Math.max(8, Math.min(right - effMinW, left + dx));
    }
    if (act.dir.includes('s')) {
      bottom = Math.min(vh - bottomSafeInset, Math.max(top + effMinH, bottom + dy));
    }
    if (act.dir.includes('n')) {
      top = Math.max(topSafeInset, Math.min(bottom - effMinH, top + dy));
    }

    setRect({
      x: Math.round(left),
      y: Math.round(top),
      width: Math.round(right - left),
      height: Math.round(bottom - top),
    });
  };

  const endAction = () => {
    actionRef.current = null;
  };

  const effectiveStyle: React.CSSProperties = isCompact
    ? { zIndex }
    : maximized
      ? {
          left: 12,
          top: topSafeInset,
          width: Math.max(320, viewport.w - 24),
          height: Math.max(260, viewport.h - topSafeInset - bottomSafeInset),
          zIndex,
        }
      : {
          left: rect.x,
          top: rect.y,
          width: rect.width,
          height: rect.height,
          zIndex,
        };

  return (
    <section
      className={isCompact ? 'floating-window compact-dock' : 'floating-window'}
      style={effectiveStyle}
      data-testid={testId}
      onPointerDown={() => onFocus?.()}
      aria-label={typeof title === 'string' ? title : '浮动工作视窗'}
    >
      <div
        className="floating-window-header"
        data-testid={`${testId}-header`}
        onPointerDown={startMove}
        onPointerMove={onPointerMove}
        onPointerUp={endAction}
        onPointerCancel={endAction}
        onDoubleClick={() => {
          if (!isCompact) toggleMaximize();
        }}
      >
        <div className="floating-window-title">{title}</div>
        <div className="floating-window-controls" data-no-window-drag="true">
          {headerActions}
          {!isCompact && (
            <button
              type="button"
              className="win-ctrl-btn"
              data-testid={`${testId}-maximize`}
              title={maximized ? '还原自由窗口大小' : '铺满当前工作区'}
              onClick={toggleMaximize}
            >
              {maximized ? '❐' : '□'}
            </button>
          )}
          {onClose && (
            <button
              type="button"
              className="win-ctrl-btn close-btn"
              data-testid={`${testId}-close`}
              title="关闭视窗（可在顶栏重新打开）"
              onClick={onClose}
            >
              ×
            </button>
          )}
        </div>
      </div>

      <div className="floating-window-body">{children}</div>

      {!isCompact &&
        RESIZE_DIRS.map((dir) => (
          <div
            key={dir}
            className={`win-resize-handle dir-${dir}`}
            data-testid={`${testId}-resize-${dir}`}
            onPointerDown={startResize(dir)}
            onPointerMove={onPointerMove}
            onPointerUp={endAction}
            onPointerCancel={endAction}
          />
        ))}
    </section>
  );
}
