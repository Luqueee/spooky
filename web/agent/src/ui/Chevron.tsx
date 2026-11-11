export function Chevron({ open }: { open: boolean }) {
  return <span className={"chevron" + (open ? " open" : "")}>▸</span>;
}
