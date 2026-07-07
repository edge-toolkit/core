using System;
using System.Runtime.InteropServices.JavaScript;
using System.Threading.Tasks;

namespace EtWsModules;

// JS-imported host functions provided by the shim
partial class Host
{
  [JSImport("wsConnect", "dotnet-data1")] internal static partial void WsConnect(string url);
  [JSImport("wsDisconnect", "dotnet-data1")] internal static partial void WsDisconnect();
  [JSImport("wsGetState", "dotnet-data1")] internal static partial string WsGetState();
  [JSImport("wsGetAgentId", "dotnet-data1")] internal static partial string WsGetAgentId();
  [JSImport("putFile", "dotnet-data1")] internal static partial Task PutFileAsync(string url, string body);
  [JSImport("getFile", "dotnet-data1")] internal static partial Task<string> GetFileAsync(string url);
  [JSImport("log", "dotnet-data1")] internal static partial void Log(string msg);
  [JSImport("setStatus", "dotnet-data1")] internal static partial void SetStatus(string msg);
  // skipcq: CS-A1000 -- [JSImport] marshals a string; System.Uri is not a supported JS-interop return type.
  [JSImport("getWsUrl", "dotnet-data1")] internal static partial string GetWsUrl();
  [JSImport("getIsoTimestamp", "dotnet-data1")] internal static partial string GetIsoTimestamp();
  [JSImport("sleep", "dotnet-data1")] internal static partial Task SleepAsync(int ms);
}

public partial class DotnetData1
{
  [JSExport]
  public static async Task RunAsync()
  {
    Host.Log("[dotnet-data1] entered Run()");
    Host.SetStatus("[dotnet-data1] entered Run()");

    Host.WsConnect(Host.GetWsUrl());
    await WaitForConnectedAsync();
    var agentId = await WaitForAgentIdAsync();

    var msg = $"[dotnet-data1] connected as {agentId}";
    Host.Log(msg);
    Host.SetStatus(msg);

    const string filename = "test_data.txt";
    var testContent = $"Hello from dotnet-data1 at {Host.GetIsoTimestamp()}!";
    var storageUrl = $"/storage/{agentId}/{filename}";

    msg = $"[dotnet-data1] storing data to {storageUrl}";
    Host.Log(msg);
    Host.SetStatus(msg);
    await Host.PutFileAsync(storageUrl, testContent);

    msg = $"[dotnet-data1] fetching data from {storageUrl}";
    Host.Log(msg);
    Host.SetStatus(msg);
    var retrieved = await Host.GetFileAsync(storageUrl);

    VerifyRoundTrip(testContent, retrieved);

    await Host.SleepAsync(2000);
    Host.WsDisconnect();
    const string done = "[dotnet-data1] workflow complete";
    Host.Log(done);
    Host.SetStatus(done);
  }

  private static async Task WaitForConnectedAsync()
  {
    for (int i = 0; i < 100; i++)
    {
      if (Host.WsGetState() == "connected")
      {
        return;
      }
      await Host.SleepAsync(100);
    }
    throw new TimeoutException("Timeout waiting for WebSocket connection");
  }

  private static async Task<string> WaitForAgentIdAsync()
  {
    for (int i = 0; i < 100; i++)
    {
      var agentId = Host.WsGetAgentId();
      if (!string.IsNullOrEmpty(agentId))
      {
        return agentId;
      }
      await Host.SleepAsync(100);
    }
    throw new TimeoutException("Timeout waiting for agent_id");
  }

  private static void VerifyRoundTrip(string sent, string retrieved)
  {
    if (retrieved == sent)
    {
      const string ok = "[dotnet-data1] VERIFICATION SUCCESS - data matches!";
      Host.Log(ok);
      Host.SetStatus(ok);
      return;
    }

    var fail = $"[dotnet-data1] VERIFICATION FAILURE\nSent: {sent}\nGot: {retrieved}";
    Host.Log(fail);
    Host.SetStatus(fail);
    throw new InvalidOperationException("Data mismatch");
  }
}
