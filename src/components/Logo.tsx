export function Logo({ size = 22 }: { size?: number }) {
  const width = Math.round(size * (572 / 490));
  return (
    <svg width={width} height={size} viewBox="226 258 572 490" aria-hidden="true">
      <g
        fill="none"
        stroke="var(--xb-logo)"
        strokeWidth="78"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M265 297H414A159 159 0 0 1 573 456V550A159 159 0 0 1 414 709H265Z" />
        <path d="M760 297V709" />
        <path d="M265 561H350C400 561 420 496 470 496H760" />
      </g>
    </svg>
  );
}
