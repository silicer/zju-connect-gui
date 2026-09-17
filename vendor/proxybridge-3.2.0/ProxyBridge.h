#ifndef PROXYBRIDGE_H
#define PROXYBRIDGE_H

#define PROXYBRIDGE_VERSION "3.2.0"

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*LogCallback)(const char* message);
typedef void (*ConnectionCallback)(const char* process_name, uint32_t pid, const char* dest_ip, uint16_t dest_port, const char* proxy_info);

typedef enum {
    PROXY_TYPE_HTTP = 0,
    PROXY_TYPE_SOCKS5 = 1
} ProxyType;

typedef enum {
    RULE_ACTION_PROXY = 0,
    RULE_ACTION_DIRECT = 1,
    RULE_ACTION_BLOCK = 2
} RuleAction;

typedef enum {
    RULE_PROTOCOL_TCP = 0,
    RULE_PROTOCOL_UDP = 1,
    RULE_PROTOCOL_BOTH = 2
} RuleProtocol;

uint32_t ProxyBridge_AddRule(const char* process_name, const char* target_hosts, const char* target_ports, RuleProtocol protocol, RuleAction action);
bool ProxyBridge_EnableRule(uint32_t rule_id);
bool ProxyBridge_DisableRule(uint32_t rule_id);
bool ProxyBridge_DeleteRule(uint32_t rule_id);
bool ProxyBridge_EditRule(uint32_t rule_id, const char* process_name, const char* target_hosts, const char* target_ports, RuleProtocol protocol, RuleAction action);
bool ProxyBridge_SetProxyConfig(ProxyType type, const char* proxy_ip, uint16_t proxy_port, const char* username, const char* password);
void ProxyBridge_SetDnsViaProxy(bool enable);
void ProxyBridge_SetLogCallback(LogCallback callback);
void ProxyBridge_SetConnectionCallback(ConnectionCallback callback);
void ProxyBridge_SetTrafficLoggingEnabled(bool enable);
void ProxyBridge_ClearConnectionLogs(void);
bool ProxyBridge_Start(void);
bool ProxyBridge_Stop(void);
int ProxyBridge_TestConnection(const char* target_host, uint16_t target_port, char* result_buffer, size_t buffer_size);

#ifdef __cplusplus
}
#endif

#endif
