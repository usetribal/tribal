export interface SessionSummary {
    id: string;
    agent: string;
    turns: number;
    started_at: string;
    model?: string;
    models_used?: string[];
    git_branch?: string;
    parent_session_id?: string;
    is_sidechain?: boolean;
    vendor_session_id?: string;
    prompted_by_email?: string;
    prompted_by_name?: string;
}

export interface ToolCall {
    name: string;
    arguments: string;
    result?: string;
}

export interface Artifact {
    kind: string;
    path: string;
    line_range?: [number, number];
    mime_type?: string;
    preview_data_url?: string;
}

export interface Turn {
    id: string;
    role: string;
    content: string;
    model?: string;
    timestamp?: string;
    tool_calls?: ToolCall[];
    artifacts?: Artifact[];
}

export interface Conversation {
    id: string;
    agent: string;
    started_at: string;
    ended_at?: string;
    workspace_root?: string;
    parent_session_id?: string;
    commit_shas?: string[];
    turns: Turn[];
    private?: boolean;
    metadata?: Record<string, unknown>;
    architecture_summary?: string;
}

export interface BlameMatch {
    turn_id: string;
    conversation_id: string;
    line_range?: [number, number];
    confidence: string;
    content_preview: string;
}

export interface BlameResult {
    line: number;
    commit_sha: string;
    line_objects: Array<{
        conversation_id: string;
        turn_id: string;
        file_path: string;
        line_range: [number, number];
        confidence: string;
    }>;
    sessions: string[];
    matches?: BlameMatch[];
}
