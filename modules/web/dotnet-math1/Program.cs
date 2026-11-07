using System;
using System.Runtime.InteropServices.JavaScript;
using System.Text.Json;
using System.Threading.Tasks;

namespace EtWsModules;

// JS-imported host functions provided by the shim
partial class Host
{
  [JSImport("wsConnect", "dotnet-math1")] internal static partial void WsConnect(string url);
  [JSImport("wsDisconnect", "dotnet-math1")] internal static partial void WsDisconnect();
  [JSImport("wsGetState", "dotnet-math1")] internal static partial string WsGetState();
  [JSImport("wsGetAgentId", "dotnet-math1")] internal static partial string WsGetAgentId();
  [JSImport("hasInput", "dotnet-math1")] internal static partial bool HasInput();
  [JSImport("fetchInputJson", "dotnet-math1")] internal static partial Task<string> FetchInputJsonAsync();
  [JSImport("putOutput", "dotnet-math1")]
  internal static partial Task PutOutputAsync(string module, double weight, double bias);
  [JSImport("log", "dotnet-math1")] internal static partial void Log(string msg);
  [JSImport("setStatus", "dotnet-math1")] internal static partial void SetStatus(string msg);
  // skipcq: CS-A1000 -- [JSImport] marshals a string; System.Uri is not a supported JS-interop return type.
  [JSImport("getWsUrl", "dotnet-math1")] internal static partial string GetWsUrl();
  [JSImport("sleep", "dotnet-math1")] internal static partial Task SleepAsync(int ms);
}

public partial class DotnetMath1
{
  [JSExport]
  public static async Task RunAsync()
  {
    Status("entered Run()");

    Host.WsConnect(Host.GetWsUrl());
    await WaitForAsync("WebSocket connection", () => Host.WsGetState() == "connected");
    await WaitForAsync("agent_id", () => !string.IsNullOrEmpty(Host.WsGetAgentId()));
    Status($"connected as {Host.WsGetAgentId()}");

    Status("waiting for the math1-input pointer broadcast");
    await WaitForAsync("math1-input pointer", Host.HasInput);
    var inputJson = await Host.FetchInputJsonAsync();
    using var input = JsonDocument.Parse(inputJson);
    var root = input.RootElement;

    var clients = root.GetProperty("clients");
    var rounds = root.GetProperty("rounds").GetInt32();
    var epochs = root.GetProperty("epochs").GetInt32();
    var learningRate = root.GetProperty("learning_rate").GetDouble();
    Status($"running FedAvg - {clients.GetArrayLength()} clients x {rounds} rounds x {epochs} local epochs");

    var (weight, bias) = FedAvg(clients, rounds, epochs, learningRate);
    Status($"global model weight={weight:R} bias={bias:R}");

    await Host.PutOutputAsync("dotnet-math1", weight, bias);
    Status("stored the global model to math1-output.json");

    await Host.SleepAsync(2000);
    Host.WsDisconnect();
    Status("workflow complete");
  }

  // Runs the FedAvg simulation on the fetched input and returns the final global (weight, bias).
  // Only + - * / on double in a fixed evaluation order, so the result is bit-identical to the
  // other math1 language twins.
  private static (double Weight, double Bias) FedAvg(
    JsonElement clients, int rounds, int epochs, double learningRate)
  {
    var weight = 0.0;
    var bias = 0.0;
    var totalSamples = 0.0;
    foreach (var samples in clients.EnumerateArray())
    {
      totalSamples += samples.GetArrayLength();
    }
    for (int round = 0; round < rounds; round++)
    {
      var mergedWeight = 0.0;
      var mergedBias = 0.0;
      foreach (var samples in clients.EnumerateArray())
      {
        double count = samples.GetArrayLength();
        var clientWeight = weight;
        var clientBias = bias;
        for (int epoch = 0; epoch < epochs; epoch++)
        {
          var gradWeight = 0.0;
          var gradBias = 0.0;
          foreach (var sample in samples.EnumerateArray())
          {
            var feature = sample[0].GetDouble();
            var target = sample[1].GetDouble();
            var residual = clientWeight * feature + clientBias - target;
            gradWeight += residual * feature;
            gradBias += residual;
          }
          clientWeight -= learningRate * (2.0 * gradWeight / count);
          clientBias -= learningRate * (2.0 * gradBias / count);
        }
        mergedWeight += clientWeight * count;
        mergedBias += clientBias * count;
      }
      weight = mergedWeight / totalSamples;
      bias = mergedBias / totalSamples;
    }
    return (weight, bias);
  }

  private static void Status(string msg)
  {
    var line = $"[dotnet-math1] {msg}";
    Host.Log(line);
    Host.SetStatus(line);
  }

  private static async Task WaitForAsync(string what, Func<bool> ready)
  {
    for (int i = 0; i < 100; i++)
    {
      if (ready())
      {
        return;
      }
      await Host.SleepAsync(100);
    }
    throw new TimeoutException($"Timeout waiting for {what}");
  }
}
