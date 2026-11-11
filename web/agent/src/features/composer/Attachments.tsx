import { post, type Attachment } from "../../bridge";

/** What the next message carries: a picture when the file is one, its name otherwise.
 * The file itself stays where it is; the message names its path. */
export function Attachments({ pane, files }: { pane: number; files: readonly Attachment[] }) {
  if (files.length === 0) return null;
  return (
    <div className="attachments">
      {files.map((file) => (
        <div key={file.named} className={"chip-file" + (file.preview ? " picture" : "")}>
          {file.preview ? (
            <img src={file.preview} alt={file.name} />
          ) : (
            <span className="file-name">{file.name}</span>
          )}
          <button
            className="drop"
            title={"Remove " + file.name}
            onClick={() => post({ type: "unattach", pane, named: file.named })}
          >
            ×
          </button>
          {file.preview && <span className="caption">{file.name}</span>}
        </div>
      ))}
    </div>
  );
}
