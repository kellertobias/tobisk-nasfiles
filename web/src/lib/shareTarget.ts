export const SHARE_TARGET_ROOT_KEY = "nasfiles-share-target-root";
export const SHARE_TARGET_PATH_KEY = "nasfiles-share-target-path";
export const PENDING_SHARE_ID_KEY = "nasfiles-pending-share-id";

const SHARE_DB_NAME = "nasfiles-share-target";
const SHARE_STORE_NAME = "incoming-shares";

export interface IncomingShareRecord {
  id: string;
  title: string;
  text: string;
  url: string;
  createdAt: number;
  files: Array<{
    name: string;
    type: string;
    lastModified: number;
    blob: Blob;
  }>;
}

export function storedShareTarget() {
  return {
    root: localStorage.getItem(SHARE_TARGET_ROOT_KEY) || "",
    path: localStorage.getItem(SHARE_TARGET_PATH_KEY) || "",
  };
}

export function saveShareTarget(root: string, path: string) {
  localStorage.setItem(SHARE_TARGET_ROOT_KEY, root);
  localStorage.setItem(SHARE_TARGET_PATH_KEY, path);
}

export function rememberPendingShareId(id: string) {
  localStorage.setItem(PENDING_SHARE_ID_KEY, id);
}

export function takePendingShareId() {
  const id = localStorage.getItem(PENDING_SHARE_ID_KEY);
  if (id) localStorage.removeItem(PENDING_SHARE_ID_KEY);
  return id;
}

export async function readIncomingShare(
  id: string,
): Promise<IncomingShareRecord | null> {
  const db = await openShareDb();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(SHARE_STORE_NAME, "readonly");
      const request = tx.objectStore(SHARE_STORE_NAME).get(id);
      request.onsuccess = () =>
        resolve((request.result as IncomingShareRecord | undefined) ?? null);
      request.onerror = () => reject(request.error);
    });
  } finally {
    db.close();
  }
}

export async function deleteIncomingShare(id: string): Promise<void> {
  const db = await openShareDb();
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(SHARE_STORE_NAME, "readwrite");
      tx.objectStore(SHARE_STORE_NAME).delete(id);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

export function filesFromIncomingShare(record: IncomingShareRecord): File[] {
  const files = record.files.map(
    (file) =>
      new File([file.blob], file.name || "shared-file", {
        type: file.type || file.blob.type,
        lastModified: file.lastModified || record.createdAt,
      }),
  );

  const sharedText = [record.title, record.text, record.url]
    .map((value) => value.trim())
    .filter(Boolean)
    .join("\n");

  if (sharedText) {
    files.push(
      new File([sharedText], "shared-text.txt", {
        type: "text/plain",
        lastModified: record.createdAt,
      }),
    );
  }

  return files;
}

function openShareDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(SHARE_DB_NAME, 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore(SHARE_STORE_NAME, { keyPath: "id" });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
