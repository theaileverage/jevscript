/**
 * The page's side of the WebSocket protocol in `shared/protocol.ts`: typed
 * requests with one reply each, and a stream of pushes from runs.
 */
import type { ClientMessage, Replies, Request, ServerMessage } from '../shared/protocol.ts'

type Push = Exclude<ServerMessage, { type: 'reply' }>

export class Api {
  #socket: WebSocket | null = null
  #next = 1
  #pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>()
  #queue: string[] = []
  #listeners = new Set<(push: Push) => void>()
  #connection = new Set<(open: boolean) => void>()

  connect(): void {
    const url = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`
    const socket = new WebSocket(url)
    this.#socket = socket
    socket.onopen = () => {
      for (const text of this.#queue.splice(0)) socket.send(text)
      for (const listener of this.#connection) listener(true)
    }
    socket.onmessage = (event) => {
      const message = JSON.parse(String(event.data)) as ServerMessage
      if (message.type === 'reply') {
        const pending = this.#pending.get(message.id)
        this.#pending.delete(message.id)
        if (message.ok) pending?.resolve(message.result)
        else pending?.reject(new Error(message.error))
        return
      }
      for (const listener of this.#listeners) listener(message)
    }
    socket.onclose = () => {
      for (const listener of this.#connection) listener(false)
      for (const pending of this.#pending.values()) pending.reject(new Error('the toolbox server went away'))
      this.#pending.clear()
      this.#queue.length = 0
      setTimeout(() => this.connect(), 1000)
    }
  }

  request<T extends Request['type']>(type: T, payload: Omit<Extract<Request, { type: T }>, 'type'>): Promise<Replies[T]> {
    const id = this.#next++
    const text = JSON.stringify({ ...payload, type, id } as ClientMessage)
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve: resolve as (value: unknown) => void, reject })
      if (this.#socket?.readyState === WebSocket.OPEN) this.#socket.send(text)
      else this.#queue.push(text)
    })
  }

  onPush(listener: (push: Push) => void): () => void {
    this.#listeners.add(listener)
    return () => this.#listeners.delete(listener)
  }

  onConnection(listener: (open: boolean) => void): () => void {
    this.#connection.add(listener)
    return () => this.#connection.delete(listener)
  }
}

export const api = new Api()
