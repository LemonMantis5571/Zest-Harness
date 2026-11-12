Configurar en los providers de CLI un MCP que se conecte a Zest y permita a los agentes conectados via CLI interactuar con acciones de Zest directamente:
- Llamar subagentes de Zest.
- Llamar MCPs a los que Zest está conectado, haciendo de puente el MCP de Zest.
- Efectuar acciones o tools nativas especificas que solo Zest implementa.


¿Cómo funciona?
- Cada agente, ejemplo, Claude Code, se configura una conexión MCP directa a Zest.
- Al arrancar, el cliente se conecta a Zest, Zest vincula la sesión actual del agente en pantalla y efectúa la conexión, todas las llamadas a MCP que haga ese agente en específico funciona bajo el contexto de la ruta actual de trabajo que Zest reconoce.
- Vinculados, el agente puede usar Zest para hacer llamadas a MCP de Zest.
