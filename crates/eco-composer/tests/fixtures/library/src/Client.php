<?php

declare(strict_types=1);

namespace Acme\Sdk;

use Acme\Sdk\Http\Request;

final class Client
{
    public function __construct(private readonly string $baseUrl)
    {
    }

    public function request(string $method, string $path): Request
    {
        return new Request($method, rtrim($this->baseUrl, '/') . '/' . ltrim($path, '/'));
    }
}
