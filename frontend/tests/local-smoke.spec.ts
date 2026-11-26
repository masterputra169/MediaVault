import { expect, test } from "@playwright/test";

test.skip(process.env.MEDIAVAULT_LOCAL_SMOKE !== "1", "Requires the local FastAPI process");

test("real Next.js and FastAPI respond over localhost with browser CORS", async ({ page }) => {
  const port = process.env.MEDIAVAULT_SMOKE_BACKEND_PORT || "8000";
  const address = "http://127.0.0.1:" + port;
  if (port !== "8000") {
    await page.route("http://127.0.0.1:8000/api/**", async (route) => {
      const url = new URL(route.request().url());
      url.port = port;
      await route.continue({ url: url.href });
    });
  }
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Analisis URL" })).toBeEnabled();
  await expect(page.getByText("Antrean kosong")).toBeVisible();
  const result = await page.evaluate(async (backend) => {
    const response = await fetch(backend + "/api/media/analyze", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url: "https://example.invalid/video" }),
    });
    return { status: response.status, body: await response.json() };
  }, address);
  expect(result.status).toBe(422);
  expect(result.body.error.code).toBe("UNSUPPORTED_PLATFORM");
  await page.getByRole("link", { name: "Pengaturan" }).click();
  await expect(page.getByText("Folder unduhan", { exact: true })).toBeVisible();
  await expect(page.getByText("Dapat ditulis")).toBeVisible();
});
