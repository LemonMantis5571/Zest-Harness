import { BrandMark } from "@/components/BrandMark";

/**
 * The chat's shape, drawn before the session exists.
 *
 * Replaces a centred spinner. A layout that is already in place and then fills
 * in reads as faster than one that appears in a single jolt, even when the wait
 * is identical — and since session start no longer blocks on the network, the
 * wait here is short enough that a spinner would only flash.
 *
 * Geometry deliberately mirrors `ChatScreen`: a 260px sidebar rail, the same
 * header height, and a composer block at the bottom, so the transition to the
 * real chat moves nothing.
 */
export function ChatSkeleton() {
  return (
    <section
      className="relative flex h-full w-full min-h-0 overflow-hidden bg-[var(--chat-canvas)]"
      // One label for the whole screen. Announcing each placeholder would be
      // noise, and the placeholders carry no information.
      role="status"
      aria-label="Opening your session"
    >
      <div className="hidden h-full w-[260px] shrink-0 flex-col gap-2 border-r border-border/60 bg-[var(--sidebar)] p-3 sm:flex">
        <div className="zest-skeleton h-7 w-full rounded-md" />
        <div className="mt-2 h-3 w-16 rounded bg-foreground/5" />
        {[88, 72, 80, 64, 76].map((width, index) => (
          <div
            key={index}
            className="zest-skeleton h-6 rounded"
            style={{ width: `${width}%`, animationDelay: `${index * 90}ms` }}
          />
        ))}
      </div>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="flex shrink-0 items-center gap-2.5 border-b border-border/60 bg-[var(--chat-header)] px-4 py-2.5">
          {/* Identity should be the one real thing on screen immediately. */}
          <BrandMark className="size-7" />
          <div className="flex flex-col gap-1.5">
            <div className="zest-skeleton h-3 w-28 rounded" />
            <div className="zest-skeleton h-2.5 w-40 rounded" />
          </div>
        </header>

        <div className="min-h-0 flex-1 overflow-hidden">
          <div className="mx-auto flex w-full max-w-[var(--chat-max)] flex-col gap-6 px-4 py-6">
            {[
              { align: "end", widths: [46] },
              { align: "start", widths: [92, 84, 61] },
              { align: "end", widths: [38] },
              { align: "start", widths: [88, 70] },
            ].map((row, rowIndex) => (
              <div
                key={rowIndex}
                className={`flex flex-col gap-2 ${row.align === "end" ? "items-end" : "items-start"}`}
              >
                {row.widths.map((width, lineIndex) => (
                  <div
                    key={lineIndex}
                    className="zest-skeleton h-3.5 rounded"
                    style={{
                      width: `${width}%`,
                      animationDelay: `${(rowIndex * 3 + lineIndex) * 70}ms`,
                    }}
                  />
                ))}
              </div>
            ))}
          </div>
        </div>

        <div className="shrink-0 px-4 pb-5">
          <div className="mx-auto w-full max-w-[var(--chat-max)]">
            <div className="zest-skeleton h-[52px] w-full rounded-xl" />
          </div>
        </div>
      </div>
    </section>
  );
}
