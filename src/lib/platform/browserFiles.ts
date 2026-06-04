/**
 * 浏览器端文件 I/O 辅助。
 *
 * Web 控制台(无 Tauri)下，桌面原生的"另存为/打开文件"对话框不可用，
 * 改用浏览器标准能力实现等价功能：
 * - 下载：Blob + 临时 <a download> 触发浏览器下载。
 * - 上传：临时 <input type="file"> 触发选择并读取文本内容。
 *
 * 仅在浏览器环境调用（isTauri() === false）。
 */

/** 将文本内容作为文件下载到浏览器。 */
export function downloadTextFile(
  filename: string,
  content: string,
  mime = "application/octet-stream",
): void {
  const blob = new Blob([content], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename || "download";
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  // 延迟回收，确保下载已发起
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * 触发浏览器文件选择并以文本读取所选文件。
 *
 * 返回所选文件的 { name, content }；用户取消则返回 null。
 * 必须在用户手势（点击）调用链中触发。
 */
export function pickTextFile(
  accept?: string,
): Promise<{ name: string; content: string } | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    if (accept) input.accept = accept;
    input.style.display = "none";

    let settled = false;
    const cleanup = () => {
      window.removeEventListener("focus", onFocus, true);
      if (input.parentNode) input.parentNode.removeChild(input);
    };
    // 用户取消对话框时无 change 事件，借窗口重新获得焦点兜底解析为 null
    const onFocus = () => {
      setTimeout(() => {
        if (!settled) {
          settled = true;
          cleanup();
          resolve(null);
        }
      }, 500);
    };

    input.addEventListener("change", () => {
      const file = input.files && input.files[0];
      if (!file) {
        if (!settled) {
          settled = true;
          cleanup();
          resolve(null);
        }
        return;
      }
      const reader = new FileReader();
      reader.onload = () => {
        settled = true;
        cleanup();
        resolve({ name: file.name, content: String(reader.result ?? "") });
      };
      reader.onerror = () => {
        settled = true;
        cleanup();
        resolve(null);
      };
      reader.readAsText(file);
    });

    document.body.appendChild(input);
    window.addEventListener("focus", onFocus, true);
    input.click();
  });
}

/**
 * 触发浏览器文件选择并以 ArrayBuffer/base64 读取（用于二进制，如 zip）。
 * 返回 { name, base64 }；取消返回 null。
 */
export function pickBinaryFileAsBase64(
  accept?: string,
): Promise<{ name: string; base64: string } | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    if (accept) input.accept = accept;
    input.style.display = "none";

    let settled = false;
    const cleanup = () => {
      window.removeEventListener("focus", onFocus, true);
      if (input.parentNode) input.parentNode.removeChild(input);
    };
    const onFocus = () => {
      setTimeout(() => {
        if (!settled) {
          settled = true;
          cleanup();
          resolve(null);
        }
      }, 500);
    };

    input.addEventListener("change", () => {
      const file = input.files && input.files[0];
      if (!file) {
        if (!settled) {
          settled = true;
          cleanup();
          resolve(null);
        }
        return;
      }
      const reader = new FileReader();
      reader.onload = () => {
        settled = true;
        cleanup();
        const result = String(reader.result ?? "");
        // dataURL 形如 data:...;base64,XXXX
        const base64 = result.includes(",") ? result.split(",")[1] : result;
        resolve({ name: file.name, base64 });
      };
      reader.onerror = () => {
        settled = true;
        cleanup();
        resolve(null);
      };
      reader.readAsDataURL(file);
    });

    document.body.appendChild(input);
    window.addEventListener("focus", onFocus, true);
    input.click();
  });
}
