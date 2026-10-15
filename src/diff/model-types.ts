// Review-comment shape. The diff wire types live in types.ts.

export type CommentSide = "old" | "new";

export interface ReviewComment {
  id: string;
  path: string;
  side: CommentSide;
  line: number;
  endLine?: number;
  text: string;
  /** The text of the commented line when the comment was made. */
  lineText: string;
  /** Diff lines around the comment (from `excerpt()`), each prefixed `+`, `-` or a space. */
  excerpt?: string[];
  /** The line could not be found again after a refresh. */
  stale?: boolean;
}
