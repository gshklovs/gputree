"""gputree launch-video stand-in: a small but real training run (MLP policy regression on
synthetic data) on the GPU. Stops after --steps or when killed."""
import argparse, time, torch
ap = argparse.ArgumentParser()
ap.add_argument("model")
ap.add_argument("--steps", type=int, default=200000)
ap.add_argument("--batch", type=int, default=16384)
ap.add_argument("--duty", type=float, default=0.8)
a = ap.parse_args()
dev = "cuda"
torch.manual_seed(0)
net = torch.nn.Sequential(torch.nn.Linear(256, 2048), torch.nn.ELU(), torch.nn.Linear(2048, 2048), torch.nn.ELU(),
                          torch.nn.Linear(2048, 2048), torch.nn.ELU(), torch.nn.Linear(2048, 32)).to(dev)
teacher = torch.nn.Sequential(torch.nn.Linear(256, 512), torch.nn.Tanh(), torch.nn.Linear(512, 32)).to(dev)
opt = torch.optim.AdamW(net.parameters(), lr=3e-4)
for step in range(a.steps):
    t0 = time.perf_counter()
    for _ in range(4):
        x = torch.randn(a.batch, 256, device=dev)
        with torch.no_grad():
            y = teacher(x)
        loss = torch.nn.functional.mse_loss(net(x), y)
        opt.zero_grad(set_to_none=True)
        loss.backward()
        opt.step()
    torch.cuda.synchronize()
    busy = time.perf_counter() - t0
    if step % 50 == 0:
        print(f"step {step} loss {loss.item():.4f}", flush=True)
    time.sleep(busy * (1 - a.duty) / a.duty)
