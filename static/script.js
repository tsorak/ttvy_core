main();

async function main() {
  const body = {
    token: location.hash.substring(1).split("&")[0].split("=")[1],
  };

  const res = await fetch("/token", {
    method: "POST",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });

  if (res.ok) {
    document.body.innerText = "yippee!";
    globalThis.close();
  }
}
