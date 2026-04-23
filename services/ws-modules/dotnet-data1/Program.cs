using System;
using System.Runtime.InteropServices.JavaScript;
using System.Threading.Tasks;

// JS-imported host functions provided by the shim
partial class Host
{
  [JSImport("wsConnect", "dotnet-data1")] internal static partial void WsConnect(string url);
  [JSImport("wsDisconnect", "dotnet-data1")] internal static partial void WsDisconnect();
  [JSImport("wsSend", "dotnet-data1")] internal static partial void WsSend(string msg);
  [JSImport("wsGetState", "dotnet-data1")] internal static partial string WsGetState();
  [JSImport("wsGetAgentId", "dotnet-data1")] internal static partial string WsGetAgentId();
  [JSImport("wsPopResponse", "dotnet-data1")] internal static partial string WsPopResponse();
  [JSImport("putFile", "dotnet-data1")] internal static partial Task PutFile(string url, string body);
  [JSImport("getFile", "dotnet-data1")] internal static partial Task<string> GetFile(string url);
  [JSImport("log", "dotnet-data1")] internal static partial void Log(string msg);
  [JSImport("setStatus", "dotnet-data1")] internal static partial void SetStatus(string msg);
  [JSImport("getWsUrl", "dotnet-data1")] internal static partial string GetWsUrl();
  [JSImport("getIsoTimestamp", "dotnet-data1")] internal static partial string GetIsoTimestamp();
  [JSImport("sleep", "dotnet-data1")] internal static partial Task Sleep(int ms);
}

public partial class DotnetData1
{
  [JSExport]
  public static async Task Run()
  {
    Host.Log("[dotnet-data1] entered Run()");
    Host.SetStatus("[dotnet-data1] entered Run()");

    var wsUrl = Host.GetWsUrl();
    Host.WsConnect(wsUrl);

    // Wait for connected
    for (int i = 0; i < 100; i++)
    {
      if (Host.WsGetState() == "connected") break;
      await Host.Sleep(100);
      if (i == 99) throw new Exception("Timeout waiting for WebSocket connection");
    }

    // Wait for agent_id
    string agentId = "";
    for (int i = 0; i < 100; i++)
    {
      agentId = Host.WsGetAgentId();
      if (!string.IsNullOrEmpty(agentId)) break;
      await Host.Sleep(100);
      if (i == 99) throw new Exception("Timeout waiting for agent_id");
    }

    var msg = $"[dotnet-data1] connected as {agentId}";
    Host.Log(msg);
    Host.SetStatus(msg);

    const string filename = "test_data.txt";
    var testContent = $"Hello from dotnet-data1 at {Host.GetIsoTimestamp()}!";

    // 1. Request store URL
    Host.Log("[dotnet-data1] requesting store URL");
    Host.WsSend($$"""{"type":"store_file","filename":"{{filename}}"}""");
    var storeUrl = await WaitForResponse("PUT to ");
    storeUrl = storeUrl.Replace("PUT to ", "");

    // 2. PUT
    msg = $"[dotnet-data1] storing data to {storeUrl}";
    Host.Log(msg);
    Host.SetStatus(msg);
    await Host.PutFile(storeUrl, testContent);

    // 3. Request fetch URL
    Host.Log("[dotnet-data1] requesting fetch URL");
    Host.WsSend($$"""{"type":"fetch_file","agent_id":"{{agentId}}","filename":"{{filename}}"}""");
    var fetchUrl = await WaitForResponse("GET from ");
    fetchUrl = fetchUrl.Replace("GET from ", "");

    // 4. GET and verify
    msg = $"[dotnet-data1] fetching data from {fetchUrl}";
    Host.Log(msg);
    Host.SetStatus(msg);
    var retrieved = await Host.GetFile(fetchUrl);

    if (retrieved == testContent)
    {
      const string ok = "[dotnet-data1] VERIFICATION SUCCESS - data matches!";
      Host.Log(ok);
      Host.SetStatus(ok);
    }
    else
    {
      var fail = $"[dotnet-data1] VERIFICATION FAILURE\nSent: {testContent}\nGot: {retrieved}";
      Host.Log(fail);
      Host.SetStatus(fail);
      throw new Exception("Data mismatch");
    }

    await Host.Sleep(2000);
    Host.WsDisconnect();
    const string done = "[dotnet-data1] workflow complete";
    Host.Log(done);
    Host.SetStatus(done);
  }

  static async Task<string> WaitForResponse(string prefix)
  {
    for (int i = 0; i < 50; i++)
    {
      var r = Host.WsPopResponse();
      if (!string.IsNullOrEmpty(r) && r.StartsWith(prefix)) return r;
      await Host.Sleep(100);
    }
    throw new Exception($"Timeout waiting for response with prefix: {prefix}");
  }
}
